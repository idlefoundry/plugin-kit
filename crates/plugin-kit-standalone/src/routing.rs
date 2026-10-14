use crate::MidiEvent;
use crossbeam_queue::ArrayQueue;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

pub const MIDI_CAPACITY: usize = 2048;
pub(crate) const MIDI_PER_BLOCK: usize = 256;
pub(crate) const CAPTURE_CAPACITY: usize = 65536;

/// Asynchronous capture clocks are reconciled with a bounded reservoir. Ratio
/// correction responds to fill error, capped at +/-1%; linear interpolation
/// needs two lookahead frames and preserves exact samples at ratio one.
pub(crate) struct InputResampler {
    queue: Arc<ArrayQueue<[f32; 2]>>,
    enabled: bool,
    prepared_frames: usize,
    primed: bool,
    current: [f32; 2],
    next: [f32; 2],
    phase: f64,
    ratio: f64,
}

impl InputResampler {
    pub(crate) fn new(
        queue: Arc<ArrayQueue<[f32; 2]>>,
        enabled: bool,
        prepared_frames: usize,
    ) -> Self {
        Self {
            queue,
            enabled,
            prepared_frames,
            primed: false,
            current: [0.0; 2],
            next: [0.0; 2],
            phase: 0.0,
            ratio: 1.0,
        }
    }
    pub(crate) fn begin(&mut self, frames: usize) {
        if !self.enabled {
            return;
        }
        let target = (self.prepared_frames.max(frames) * 2).clamp(4, self.queue.capacity() / 2);
        if !self.primed {
            if self.queue.len() < target {
                return;
            }
            let Some(current) = self.queue.pop() else {
                return;
            };
            let Some(next) = self.queue.pop() else {
                return;
            };
            self.current = current;
            self.next = next;
            self.phase = 0.0;
            self.primed = true;
        }
        let fill = self.queue.len() + 2;
        self.ratio =
            1.0 + ((fill as f64 - target as f64) / (target as f64 * 100.0)).clamp(-0.01, 0.01);
    }
    /// The boolean marks a genuine underflow after priming, not startup silence.
    pub(crate) fn sample(&mut self) -> ([f32; 2], bool) {
        if !self.enabled || !self.primed {
            return ([0.0; 2], false);
        }
        let output = if self.phase == 0.0 {
            self.current
        } else {
            std::array::from_fn(|channel| {
                ((1.0 - self.phase) * f64::from(self.current[channel])
                    + self.phase * f64::from(self.next[channel])) as f32
            })
        };
        self.phase += self.ratio;
        // ratio is <=1.01 and phase before addition is <1: at most two frames.
        for _ in 0..2 {
            if self.phase < 1.0 {
                break;
            }
            self.phase -= 1.0;
            self.current = self.next;
            let Some(next) = self.queue.pop() else {
                self.primed = false;
                self.current = [0.0; 2];
                self.next = [0.0; 2];
                self.phase = 0.0;
                return (output, true);
            };
            self.next = next;
        }
        (output, false)
    }
}

#[derive(Debug)]
pub struct MidiQueue {
    queue: ArrayQueue<MidiEvent>,
    panic: AtomicBool,
    overflows: AtomicU64,
}

impl Default for MidiQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl MidiQueue {
    pub fn new() -> Self {
        Self {
            queue: ArrayQueue::new(MIDI_CAPACITY),
            panic: AtomicBool::new(false),
            overflows: AtomicU64::new(0),
        }
    }
    pub fn push(&self, event: MidiEvent) -> bool {
        if event.source > 64
            || !(1..=3).contains(&event.len)
            || event.data[0] < 0x80
            || event.data[0] >= 0xf0
            || event.data[1..usize::from(event.len)]
                .iter()
                .any(|v| *v >= 0x80)
        {
            return false;
        }
        let expected = if matches!(event.data[0] & 0xf0, 0xc0 | 0xd0) {
            2
        } else {
            3
        };
        if event.len != expected {
            return false;
        }
        if self.queue.push(event).is_err() {
            self.overflows.fetch_add(1, Ordering::Relaxed);
            self.panic.store(true, Ordering::Release);
            false
        } else {
            true
        }
    }
    pub fn panic(&self) {
        self.panic.store(true, Ordering::Release);
    }
    pub fn overflows(&self) -> u64 {
        self.overflows.load(Ordering::Relaxed)
    }
    pub(crate) fn take_panic(&self) -> bool {
        if !self.panic.swap(false, Ordering::AcqRel) {
            return false;
        }
        // A fixed upper bound even while producers continue; stale queued note-ons
        // must not follow an emergency release.
        for _ in 0..MIDI_CAPACITY {
            if self.queue.pop().is_none() {
                break;
            }
        }
        true
    }
    pub(crate) fn pop(&self) -> Option<MidiEvent> {
        self.queue.pop()
    }
}

/// One engine note is held while any selected device/channel or local source
/// owns that pitch. Repeated note-ons from the same owner do not retrigger.
pub(crate) struct MidiRouter {
    held: Vec<bool>,
    counts: [u16; 128],
    output_channel: [u8; 128],
    channel: Option<u8>,
}

impl MidiRouter {
    pub(crate) fn new(channel: Option<u8>) -> Self {
        Self {
            held: vec![false; 65 * 16 * 128],
            counts: [0; 128],
            output_channel: [0; 128],
            channel,
        }
    }
    pub(crate) fn clear(&mut self) {
        self.held.fill(false);
        self.counts.fill(0);
    }
    pub(crate) fn route(&mut self, mut event: MidiEvent) -> Option<MidiEvent> {
        let channel = event.data[0] & 15;
        if event.source != 0 && self.channel.is_some_and(|selected| selected != channel + 1) {
            return None;
        }
        let kind = event.data[0] & 0xf0;
        if kind == 0xb0 && matches!(event.data[1], 120 | 123) {
            self.clear();
            return Some(event);
        }
        if kind != 0x80 && kind != 0x90 {
            return Some(event);
        }
        if event.len != 3 {
            return None;
        }
        let note = usize::from(event.data[1]);
        let owner = (usize::from(event.source) * 16 + usize::from(channel)) * 128 + note;
        let on = kind == 0x90 && event.data[2] != 0;
        if on {
            if self.held[owner] {
                return None;
            }
            self.held[owner] = true;
            self.counts[note] += 1;
            if self.counts[note] > 1 {
                return None;
            }
            self.output_channel[note] = channel;
        } else {
            if !self.held[owner] {
                return None;
            }
            self.held[owner] = false;
            self.counts[note] -= 1;
            if self.counts[note] != 0 {
                return None;
            }
            event.data[0] = 0x80 | self.output_channel[note];
        }
        Some(event)
    }
}

fn finite(value: f32) -> f32 {
    if value.is_finite() { value } else { 0.0 }
}

/// Deinterleave selected channels, duplicate mono, and replace NaN/Inf with zero.
pub fn sanitize_input(
    input: &[f32],
    channels: usize,
    selected: [usize; 2],
    left: &mut [f32],
    right: &mut [f32],
) -> bool {
    left.fill(0.0);
    right.fill(0.0);
    if channels == 0
        || selected.into_iter().any(|c| c >= channels)
        || left.len() != right.len()
        || input.len() != left.len().saturating_mul(channels)
    {
        return false;
    }
    for (frame, (l, r)) in left.iter_mut().zip(right).enumerate() {
        *l = finite(input[frame * channels + selected[0]]);
        *r = finite(input[frame * channels + selected[1]]);
    }
    true
}

/// Fill all hardware channels with zero, then write the selected pair. Equal
/// selections use (L+R)/2; no channel is accidentally written twice.
pub fn route_output(
    left: &[f32],
    right: &[f32],
    output: &mut [f32],
    channels: usize,
    selected: [usize; 2],
) -> bool {
    output.fill(0.0);
    if channels == 0
        || selected.into_iter().any(|c| c >= channels)
        || left.len() != right.len()
        || output.len() != left.len().saturating_mul(channels)
    {
        return false;
    }
    for (frame, (l, r)) in left.iter().zip(right).enumerate() {
        if selected[0] == selected[1] {
            output[frame * channels + selected[0]] = finite(*l * 0.5 + *r * 0.5);
        } else {
            output[frame * channels + selected[0]] = finite(*l);
            output[frame * channels + selected[1]] = finite(*r);
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    fn event(source: u16, channel: u8, on: bool) -> MidiEvent {
        MidiEvent {
            source,
            data: [
                (if on { 0x90 } else { 0x80 }) | channel,
                60,
                if on { 100 } else { 0 },
            ],
            len: 3,
        }
    }
    #[test]
    fn independent_sources_channels_and_repeats_do_not_release_each_other() {
        let mut r = MidiRouter::new(None);
        assert!(r.route(event(1, 0, true)).is_some());
        assert!(r.route(event(1, 0, true)).is_none());
        assert!(r.route(event(2, 0, true)).is_none());
        assert!(r.route(event(0, 1, true)).is_none());
        assert!(r.route(event(1, 0, false)).is_none());
        assert!(r.route(event(2, 0, false)).is_none());
        assert!(r.route(event(0, 1, false)).is_some());
    }
    #[test]
    fn velocity_zero_and_channel_filter() {
        let mut r = MidiRouter::new(Some(2));
        assert!(r.route(event(1, 0, true)).is_none());
        assert!(r.route(event(1, 1, true)).is_some());
        let mut off = event(1, 1, true);
        off.data[2] = 0;
        assert_eq!(r.route(off).expect("off").data[0], 0x81);
        assert!(r.route(event(0, 0, true)).is_some());
    }
    #[test]
    fn queue_overflow_requests_release_and_discards_stale_events() {
        let q = MidiQueue::new();
        for _ in 0..MIDI_CAPACITY {
            assert!(q.push(event(1, 0, true)));
        }
        assert!(!q.push(event(1, 0, false)));
        assert_eq!(q.overflows(), 1);
        assert!(q.take_panic());
        assert!(q.pop().is_none());
        assert!(!q.take_panic());
    }
    #[test]
    fn concurrent_producers_do_not_overwrite_each_other() {
        let queue = std::sync::Arc::new(MidiQueue::new());
        std::thread::scope(|scope| {
            for source in 1..=4 {
                let q = std::sync::Arc::clone(&queue);
                scope.spawn(move || {
                    for _ in 0..512 {
                        assert!(q.push(event(source, 0, true)));
                    }
                });
            }
        });
        let mut counts = [0; 4];
        for _ in 0..MIDI_CAPACITY {
            let event = queue.pop().expect("queued");
            counts[usize::from(event.source) - 1] += 1;
        }
        assert_eq!(counts, [512; 4]);
        assert_eq!(queue.overflows(), 0);
        assert!(queue.pop().is_none());
    }
    #[test]
    fn all_channel_counts_mapping_downmix_and_sanitization() {
        for channels in [1, 2, 4, 8] {
            let mut output = vec![99.0; 3 * channels];
            let selection = [0, channels - 1];
            assert!(route_output(
                &[1.0, 2.0, f32::INFINITY],
                &[3.0, 4.0, f32::NAN],
                &mut output,
                channels,
                selection
            ));
            assert_eq!(output[0], if channels == 1 { 2.0 } else { 1.0 });
            for frame in output.chunks(channels) {
                for (i, v) in frame.iter().enumerate() {
                    assert!(v.is_finite());
                    if !selection.contains(&i) {
                        assert_eq!(*v, 0.0);
                    }
                }
            }
            let mut l = [7.0; 3];
            let mut r = [7.0; 3];
            assert!(sanitize_input(&output, channels, [0, 0], &mut l, &mut r));
            assert_eq!(l, r);
            assert!(!sanitize_input(
                &output,
                channels,
                [channels, 0],
                &mut l,
                &mut r
            ));
            assert_eq!(l, [0.0; 3]);
        }
        let mut l = [0.0; 2];
        let mut r = [0.0; 2];
        assert!(sanitize_input(
            &[f32::NAN, 1.0, f32::INFINITY, f32::NEG_INFINITY],
            2,
            [0, 1],
            &mut l,
            &mut r
        ));
        assert_eq!(l, [0.0, 0.0]);
        assert_eq!(r, [1.0, 0.0]);
    }
    #[test]
    fn independent_capture_clocks_stay_bounded_for_sixty_seconds() {
        for period in [128, 256] {
            for ppm in [-500.0, 500.0] {
                let queue = Arc::new(ArrayQueue::new(CAPTURE_CAPACITY));
                let mut input = InputResampler::new(Arc::clone(&queue), true, period);
                let ratio = 1.0 + ppm / 1_000_000.0;
                let mut pending = 0.0;
                let mut underflows = 0;
                let mut min_fill = usize::MAX;
                let mut max_fill = 0;
                for block in 0..(48000 * 60 / period) {
                    pending += period as f64 * ratio;
                    let frames = pending.floor() as usize;
                    pending -= frames as f64;
                    for _ in 0..frames {
                        assert!(
                            queue.push([0.5, -0.5]).is_ok(),
                            "capture overflow {period}/{ppm}"
                        );
                    }
                    input.begin(period);
                    for _ in 0..period {
                        let (sample, underflow) = input.sample();
                        underflows += usize::from(underflow);
                        assert!(sample.iter().all(|v| v.is_finite()));
                    }
                    if block > 10 {
                        min_fill = min_fill.min(queue.len());
                        max_fill = max_fill.max(queue.len());
                    }
                }
                assert_eq!(underflows, 0, "capture underflow {period}/{ppm}");
                assert!(
                    min_fill > period / 2,
                    "reservoir drained {period}/{ppm}: {min_fill}"
                );
                assert!(
                    max_fill < period * 2,
                    "reservoir grew {period}/{ppm}: {max_fill}"
                );
            }
        }
    }
    #[test]
    fn equal_clocks_copy_samples_exactly_and_disabled_input_is_silent() {
        let queue = Arc::new(ArrayQueue::new(64));
        let mut input = InputResampler::new(Arc::clone(&queue), true, 8);
        for sample in 0..16 {
            queue
                .push([sample as f32, -(sample as f32)])
                .expect("input");
        }
        input.begin(8);
        for sample in 0..8 {
            assert_eq!(input.sample(), ([sample as f32, -(sample as f32)], false));
        }
        for sample in 16..24 {
            queue
                .push([sample as f32, -(sample as f32)])
                .expect("input");
        }
        input.begin(8);
        for sample in 8..16 {
            assert_eq!(input.sample(), ([sample as f32, -(sample as f32)], false));
        }
        let mut disabled = InputResampler::new(queue, false, 8);
        disabled.begin(8);
        assert_eq!(disabled.sample(), ([0.0; 2], false));
    }
}
