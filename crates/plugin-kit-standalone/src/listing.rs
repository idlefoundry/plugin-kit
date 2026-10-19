//! Device discovery off the control thread. A driver call cannot be interrupted,
//! so it is made by a worker the control thread can walk away from: the control
//! loop only sends requests and reads results without waiting, and a device that
//! does not answer within the time limit is named and left out instead of holding
//! the audio list, MIDI and every command behind it (CA-72 issue 112).
use crate::devices::{AudioListing, AudioSource, DeviceInfo};
use crossbeam_channel::{Receiver, Sender, TryRecvError, bounded, unbounded};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

/// How long one device may take to answer before it is named as not responding.
pub(crate) const DEVICE_LIMIT: Duration = Duration::from_millis(1500);
/// Live workers left behind inside a call. A named device holds at most one of
/// them (later rounds skip it); this caps the rest, such as a backend whose
/// device list itself never returns.
const MAX_ABANDONED: usize = 8;

enum Event {
    /// The worker is about to wait on this device (None: before its name is known).
    Probing(Option<String>),
    Listed(DeviceInfo),
    Skipped,
    /// Still blocked in an earlier, abandoned worker's call.
    Unresponsive(String),
    Backend {
        name: String,
        default_output: Option<String>,
    },
    Done,
    Failed(String),
}

type RunWorker = Arc<dyn Fn(usize, Receiver<()>, Sender<Event>) + Send + Sync>;

/// Device names some worker is waiting on, with that worker's number. A worker
/// skips a name another worker is still waiting on, so each blocked device holds
/// at most one thread however many rounds pass.
#[derive(Clone, Default)]
struct InFlight(Arc<Mutex<Vec<(usize, String)>>>);

impl InFlight {
    fn lock(&self) -> MutexGuard<'_, Vec<(usize, String)>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
    fn held_by_other(&self, worker: usize, name: &str) -> bool {
        self.lock().iter().any(|(w, n)| *w != worker && n == name)
    }
}

fn work<S: AudioSource>(
    source: Result<S, String>,
    worker: usize,
    requests: Receiver<()>,
    events: Sender<Event>,
    in_flight: InFlight,
) {
    // A failed send means the control thread abandoned this worker: stop.
    while requests.recv().is_ok() {
        let source = match &source {
            Ok(source) => source,
            Err(error) => {
                if events.send(Event::Failed(error.clone())).is_err() {
                    return;
                }
                continue;
            }
        };
        if events.send(Event::Probing(None)).is_err() {
            return;
        }
        let devices = match source.devices() {
            Ok(devices) => devices,
            Err(error) => {
                if events.send(Event::Failed(error)).is_err() {
                    return;
                }
                continue;
            }
        };
        let header = Event::Backend {
            name: source.backend(),
            default_output: source.default_output_name(),
        };
        if events.send(header).is_err() {
            return;
        }
        for device in &devices {
            if events.send(Event::Probing(None)).is_err() {
                return;
            }
            let Ok(name) = source.name(device) else {
                if events.send(Event::Skipped).is_err() {
                    return;
                }
                continue;
            };
            if in_flight.held_by_other(worker, &name) {
                if events.send(Event::Unresponsive(name)).is_err() {
                    return;
                }
                continue;
            }
            in_flight.lock().push((worker, name.clone()));
            if events.send(Event::Probing(Some(name.clone()))).is_err() {
                in_flight.lock().retain(|(w, _)| *w != worker);
                return;
            }
            let described = source.describe(device, name);
            in_flight.lock().retain(|(w, _)| *w != worker);
            if events
                .send(described.map_or(Event::Skipped, Event::Listed))
                .is_err()
            {
                return;
            }
        }
        if events.send(Event::Done).is_err() {
            return;
        }
    }
}

struct Worker {
    requests: Sender<()>,
    events: Receiver<Event>,
}

struct Round {
    listing: AudioListing,
    probing: Option<Option<String>>,
    last_event: Instant,
}

/// What a poll of the audio listing produced for the control thread.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Listed {
    /// A complete round: every device either answered or is named unresponsive.
    Round(AudioListing),
    /// The backend could not list its devices.
    Failed(String),
}

/// The control thread's side of the audio listing. Every method returns at once.
pub(crate) struct Lister {
    run: RunWorker,
    limit: Duration,
    worker: Option<Worker>,
    round: Option<Round>,
    /// Names abandoned during the current round, reported with its result.
    stalled: Vec<String>,
    /// Worker threads that have not returned, the current one included.
    alive: Arc<AtomicUsize>,
    next_worker: usize,
}

impl Lister {
    /// `make` builds the source on each worker's own thread, so it need not be Send.
    pub(crate) fn new<S, F>(make: F, limit: Duration) -> Self
    where
        S: AudioSource + 'static,
        F: Fn() -> Result<S, String> + Send + Sync + 'static,
    {
        let in_flight = InFlight::default();
        Self {
            run: Arc::new(move |worker, requests, events| {
                work(make(), worker, requests, events, in_flight.clone());
            }),
            limit,
            worker: None,
            round: None,
            stalled: Vec::new(),
            alive: Arc::new(AtomicUsize::new(0)),
            next_worker: 0,
        }
    }

    fn spawn(&mut self) -> bool {
        let abandoned = self
            .alive
            .load(Ordering::Acquire)
            .saturating_sub(usize::from(self.worker.is_some()));
        if abandoned >= MAX_ABANDONED {
            return false;
        }
        let (requests, requested) = bounded(1);
        let (sent, events) = unbounded();
        let run = Arc::clone(&self.run);
        let alive = Arc::clone(&self.alive);
        let number = self.next_worker;
        self.next_worker += 1;
        alive.fetch_add(1, Ordering::AcqRel);
        let spawned = std::thread::Builder::new()
            .name("standalone device listing".into())
            .spawn(move || {
                // Counted down however the worker ends, a panic included.
                struct Alive(Arc<AtomicUsize>);
                impl Drop for Alive {
                    fn drop(&mut self) {
                        self.0.fetch_sub(1, Ordering::AcqRel);
                    }
                }
                let _alive = Alive(alive);
                run(number, requested, sent);
            });
        if spawned.is_err() {
            self.alive.fetch_sub(1, Ordering::AcqRel);
            return false;
        }
        self.worker = Some(Worker { requests, events });
        true
    }

    /// Start a round unless one is already running.
    pub(crate) fn request(&mut self) {
        if self.round.is_some() {
            return;
        }
        if self.worker.is_none() && !self.spawn() {
            return;
        }
        let Some(worker) = &self.worker else { return };
        if worker.requests.try_send(()).is_ok() {
            self.round = Some(Round {
                listing: AudioListing::default(),
                probing: None,
                last_event: Instant::now(),
            });
        }
    }

    /// Read what the worker has sent. A device that has kept the worker waiting
    /// longer than the limit is named, its worker is left behind and the round
    /// starts again on a new worker, which skips that device until it answers.
    pub(crate) fn poll(&mut self) -> Option<Listed> {
        loop {
            let (Some(worker), Some(round)) = (&self.worker, &mut self.round) else {
                return None;
            };
            match worker.events.try_recv() {
                Ok(event) => {
                    round.last_event = Instant::now();
                    match event {
                        Event::Probing(name) => round.probing = Some(name),
                        Event::Listed(info) => {
                            round.probing = None;
                            round.listing.audio.push(info);
                        }
                        Event::Skipped => round.probing = None,
                        Event::Unresponsive(name) => {
                            round.probing = None;
                            round.listing.unresponsive.push(name);
                        }
                        Event::Backend {
                            name,
                            default_output,
                        } => {
                            round.probing = None;
                            round.listing.backend = name;
                            round.listing.default_output_name = default_output;
                        }
                        Event::Done => {
                            let mut round = self.round.take().expect("round in progress");
                            for name in self.stalled.drain(..) {
                                if !round.listing.unresponsive.contains(&name) {
                                    round.listing.unresponsive.push(name);
                                }
                            }
                            round.listing.unresponsive.sort();
                            return Some(Listed::Round(round.listing));
                        }
                        Event::Failed(error) => {
                            self.round = None;
                            self.stalled.clear();
                            return Some(Listed::Failed(error));
                        }
                    }
                }
                Err(TryRecvError::Empty) => {
                    if round.last_event.elapsed() < self.limit {
                        return None;
                    }
                    let name = round
                        .probing
                        .clone()
                        .flatten()
                        .unwrap_or_else(|| "An audio device".into());
                    if !self.stalled.contains(&name) {
                        self.stalled.push(name);
                    }
                    let partial = self.round.take().expect("round in progress").listing;
                    self.worker = None;
                    if !self.spawn() {
                        // Too many calls are still blocked: publish what answered
                        // and try again on a later request.
                        let mut listing = partial;
                        listing.unresponsive.append(&mut self.stalled);
                        listing.unresponsive.sort();
                        listing.unresponsive.dedup();
                        return Some(Listed::Round(listing));
                    }
                    self.request();
                }
                Err(TryRecvError::Disconnected) => {
                    self.worker = None;
                    self.round = None;
                    return Some(Listed::Failed(
                        "The device listing stopped unexpectedly.".into(),
                    ));
                }
            }
        }
    }
}

/// The control thread's side of MIDI discovery, on a worker of its own so that
/// MIDI ports are listed whatever the audio devices do.
pub(crate) struct MidiPoller {
    requests: Sender<()>,
    results: Receiver<Result<Vec<String>, String>>,
    pending: Option<Instant>,
    limit: Duration,
    /// A stall or a stopped worker is reported once, not on every poll.
    reported: bool,
}

impl MidiPoller {
    /// `make` builds the poll function on the worker's thread, where the MIDI
    /// client then lives for the worker's life.
    pub(crate) fn spawn<F, P>(make: F, limit: Duration) -> Self
    where
        F: FnOnce() -> P + Send + 'static,
        P: FnMut() -> Result<Vec<String>, String>,
    {
        let (requests, requested) = bounded::<()>(1);
        let (sent, results) = bounded(1);
        // A thread that cannot start drops both ends; `poll` then reports it.
        let _ = std::thread::Builder::new()
            .name("standalone MIDI discovery".into())
            .spawn(move || {
                let mut poll = make();
                while requested.recv().is_ok() {
                    if sent.send(poll()).is_err() {
                        return;
                    }
                }
            });
        Self {
            requests,
            results,
            pending: None,
            limit,
            reported: false,
        }
    }

    pub(crate) fn request(&mut self) {
        if self.pending.is_none() && self.requests.try_send(()).is_ok() {
            self.pending = Some(Instant::now());
        }
    }

    /// A result, or once per stall an error naming it; the previous ports stay.
    pub(crate) fn poll(&mut self) -> Option<Result<Vec<String>, String>> {
        match self.results.try_recv() {
            Ok(result) => {
                self.pending = None;
                self.reported = false;
                Some(result)
            }
            Err(TryRecvError::Disconnected) => (!std::mem::replace(&mut self.reported, true))
                .then(|| Err("MIDI discovery stopped.".into())),
            Err(TryRecvError::Empty) => {
                let stalled = self
                    .pending
                    .is_some_and(|since| since.elapsed() >= self.limit);
                (stalled && !std::mem::replace(&mut self.reported, true))
                    .then(|| Err("MIDI isn't responding.".into()))
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::devices::{DeviceId, StreamCapability};
    use std::sync::atomic::AtomicUsize;

    pub(crate) fn info(name: &str, inputs: u16, outputs: u16) -> DeviceInfo {
        let capability = |channels| StreamCapability {
            channels,
            min_sample_rate: 44100,
            max_sample_rate: 96000,
            buffer_range: Some([16, 4096]),
        };
        DeviceInfo {
            id: DeviceId {
                name: name.into(),
                input_channels: inputs,
                output_channels: outputs,
            },
            input_configs: (inputs > 0)
                .then(|| capability(inputs))
                .into_iter()
                .collect(),
            output_configs: (outputs > 0)
                .then(|| capability(outputs))
                .into_iter()
                .collect(),
            sample_rates: vec![44100, 48000, 96000],
            default_sample_rate: 48000,
        }
    }

    /// A device of the fake host. A blocking call waits until the test drops the
    /// matching sender, as a driver that never answers would wait forever.
    #[derive(Clone)]
    pub(crate) struct FakeDevice {
        pub(crate) info: DeviceInfo,
        pub(crate) name_blocks: Option<Receiver<()>>,
        pub(crate) describe_blocks: Option<Receiver<()>>,
    }

    impl FakeDevice {
        pub(crate) fn answers(info: DeviceInfo) -> Self {
            Self {
                info,
                name_blocks: None,
                describe_blocks: None,
            }
        }
        pub(crate) fn blocks(info: DeviceInfo, until: Receiver<()>) -> Self {
            Self {
                describe_blocks: Some(until),
                ..Self::answers(info)
            }
        }
    }

    #[derive(Clone)]
    pub(crate) struct FakeSource {
        pub(crate) devices: Vec<FakeDevice>,
        pub(crate) default_output: Option<String>,
        /// Describe calls made on blocking devices, whether or not they returned.
        pub(crate) blocked_calls: Arc<AtomicUsize>,
    }

    impl FakeSource {
        pub(crate) fn new(devices: Vec<FakeDevice>, default_output: Option<&str>) -> Self {
            Self {
                devices,
                default_output: default_output.map(Into::into),
                blocked_calls: Arc::default(),
            }
        }
    }

    impl AudioSource for FakeSource {
        type Device = FakeDevice;
        fn backend(&self) -> String {
            "Fake".into()
        }
        fn devices(&self) -> Result<Vec<FakeDevice>, String> {
            Ok(self.devices.clone())
        }
        fn default_output_name(&self) -> Option<String> {
            self.default_output.clone()
        }
        fn name(&self, device: &FakeDevice) -> Result<String, String> {
            if let Some(until) = &device.name_blocks {
                let _ = until.recv();
            }
            Ok(device.info.id.name.clone())
        }
        fn describe(&self, device: &FakeDevice, _name: String) -> Result<DeviceInfo, String> {
            if let Some(until) = &device.describe_blocks {
                self.blocked_calls.fetch_add(1, Ordering::SeqCst);
                let _ = until.recv();
            }
            Ok(device.info.clone())
        }
    }

    pub(crate) fn lister(source: &FakeSource, limit: Duration) -> Lister {
        let source = source.clone();
        Lister::new(move || Ok(source.clone()), limit)
    }

    /// Poll as the control loop does; return the result and the longest poll.
    fn round(lister: &mut Lister, timeout: Duration) -> (AudioListing, Duration, Duration) {
        let start = Instant::now();
        let mut longest = Duration::ZERO;
        lister.request();
        loop {
            let before = Instant::now();
            let result = lister.poll();
            longest = longest.max(before.elapsed());
            match result {
                Some(Listed::Round(listing)) => return (listing, start.elapsed(), longest),
                Some(Listed::Failed(error)) => panic!("listing failed: {error}"),
                None => {}
            }
            assert!(start.elapsed() < timeout, "no listing within {timeout:?}");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn names(listing: &AudioListing) -> Vec<&str> {
        listing.audio.iter().map(|d| d.id.name.as_str()).collect()
    }

    #[test]
    fn a_device_that_never_answers_is_named_and_every_other_device_is_listed() {
        let limit = Duration::from_millis(150);
        let (release, until) = bounded::<()>(0);
        let source = FakeSource::new(
            vec![
                FakeDevice::answers(info("Built-in Output", 0, 2)),
                FakeDevice::blocks(info("Steam Streaming Speakers", 0, 2), until),
                FakeDevice::answers(info("Interface", 2, 8)),
            ],
            Some("Interface"),
        );
        let mut lister = lister(&source, limit);

        let (listing, took, longest) = round(&mut lister, Duration::from_secs(5));
        assert_eq!(names(&listing), ["Built-in Output", "Interface"]);
        assert_eq!(listing.unresponsive, ["Steam Streaming Speakers"]);
        assert_eq!(listing.default_output_name.as_deref(), Some("Interface"));
        assert_eq!(listing.backend, "Fake");
        assert!(
            took >= limit && took < limit * 4,
            "bounded by the limit: {took:?}"
        );
        assert!(
            longest < Duration::from_millis(50),
            "poll waited {longest:?}"
        );

        // Later rounds skip the device while its first call is still blocked:
        // no second thread waits on it, and they finish without the limit.
        for _ in 0..3 {
            let (listing, took, _) = round(&mut lister, Duration::from_secs(5));
            assert_eq!(names(&listing), ["Built-in Output", "Interface"]);
            assert_eq!(listing.unresponsive, ["Steam Streaming Speakers"]);
            assert!(took < limit, "skipped at once: {took:?}");
        }
        assert_eq!(source.blocked_calls.load(Ordering::SeqCst), 1);

        // Once the driver answers, the device is listed again.
        drop(release);
        let start = Instant::now();
        loop {
            let (listing, _, _) = round(&mut lister, Duration::from_secs(5));
            if listing.unresponsive.is_empty() {
                assert_eq!(
                    names(&listing),
                    ["Built-in Output", "Steam Streaming Speakers", "Interface"]
                );
                break;
            }
            assert!(start.elapsed() < Duration::from_secs(5), "never recovered");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn several_stalled_devices_are_all_named_and_the_default_answers() {
        let limit = Duration::from_millis(100);
        let (release, until) = bounded::<()>(0);
        let stalled = [
            "Immersed",
            "Steam Streaming Microphone",
            "Steam Streaming Speakers",
            "Virtual Desktop Mic",
            "Virtual Desktop Speakers",
        ];
        let mut devices: Vec<_> = stalled
            .iter()
            .map(|name| FakeDevice::blocks(info(name, 2, 2), until.clone()))
            .collect();
        devices.insert(2, FakeDevice::answers(info("MacBook Pro Speakers", 0, 2)));
        let source = FakeSource::new(devices, Some("MacBook Pro Speakers"));
        let mut lister = lister(&source, limit);
        let (listing, took, _) = round(&mut lister, Duration::from_secs(5));
        assert_eq!(names(&listing), ["MacBook Pro Speakers"]);
        assert_eq!(listing.unresponsive, stalled);
        assert!(took < limit * 7, "one limit per stalled device: {took:?}");
        assert_eq!(source.blocked_calls.load(Ordering::SeqCst), stalled.len());
        drop(release);
    }

    #[test]
    fn a_device_whose_name_never_answers_is_bounded_too() {
        let limit = Duration::from_millis(20);
        let (release, until) = bounded::<()>(0);
        let mut nameless = FakeDevice::answers(info("Hidden", 0, 2));
        nameless.name_blocks = Some(until);
        let source = FakeSource::new(
            vec![FakeDevice::answers(info("Speakers", 0, 2)), nameless],
            Some("Speakers"),
        );
        let mut lister = lister(&source, limit);
        let (listing, took, longest) = round(&mut lister, Duration::from_secs(5));
        assert_eq!(names(&listing), ["Speakers"]);
        assert_eq!(listing.unresponsive, ["An audio device"]);
        assert!(took < Duration::from_secs(2), "capped: {took:?}");
        assert!(
            longest < Duration::from_millis(50),
            "poll waited {longest:?}"
        );
        drop(release);
    }

    #[test]
    fn midi_that_never_answers_is_reported_once_and_recovers() {
        let (release, until) = bounded::<()>(0);
        let mut midi = MidiPoller::spawn(
            move || {
                let mut first = true;
                move || {
                    if std::mem::take(&mut first) {
                        let _ = until.recv();
                    }
                    Ok(vec!["Keys".into()])
                }
            },
            Duration::from_millis(50),
        );
        midi.request();
        let start = Instant::now();
        let stalled = loop {
            if let Some(result) = midi.poll() {
                break result;
            }
            assert!(start.elapsed() < Duration::from_secs(2));
            std::thread::sleep(Duration::from_millis(2));
        };
        assert_eq!(stalled, Err("MIDI isn't responding.".to_string()));
        std::thread::sleep(Duration::from_millis(60));
        assert_eq!(midi.poll(), None, "reported once");
        drop(release);
        let start = Instant::now();
        loop {
            midi.request();
            if let Some(result) = midi.poll() {
                assert_eq!(result, Ok(vec!["Keys".to_string()]));
                break;
            }
            assert!(start.elapsed() < Duration::from_secs(2));
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// The real host, on a Mac: listing reads properties only. An AudioUnit
    /// would give each device an IO proc, which asks for microphone access for
    /// an input and never returns on some virtual drivers (CA-72 issue 112).
    #[cfg(target_os = "macos")]
    #[test]
    fn listing_mac_devices_makes_no_audio_unit() {
        let before = cpal::coreaudio_audio_units_made();
        let mut lister = Lister::new(
            || Ok(crate::devices::CpalSource(cpal::default_host())),
            DEVICE_LIMIT,
        );
        let (listing, took, _) = round(&mut lister, Duration::from_secs(20));
        assert_eq!(cpal::coreaudio_audio_units_made(), before);
        assert!(!listing.audio.is_empty(), "a Mac has an output");
        assert!(listing.audio.iter().any(|d| d.id.output_channels > 0));
        assert!(
            listing
                .audio
                .iter()
                .flat_map(|d| d.input_configs.iter().chain(&d.output_configs))
                .all(|c| c.buffer_range.is_some()),
            "buffer ranges read from the devices"
        );
        eprintln!(
            "listed {} devices in {took:?} with no AudioUnit; unresponsive: {:?}",
            listing.audio.len(),
            listing.unresponsive
        );
        for device in &listing.audio {
            eprintln!(
                "  {} in {} out {} rates {:?} default {}",
                device.id.name,
                device.id.input_channels,
                device.id.output_channels,
                device.sample_rates,
                device.default_sample_rate
            );
        }
    }
}
