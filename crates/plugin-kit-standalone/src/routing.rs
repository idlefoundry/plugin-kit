use crate::{MidiEvent, MidiResetScope};
use plugin_kit_queue::AtomicQueue;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

pub const MIDI_CAPACITY: usize = 2048;
pub(crate) const MIDI_PER_BLOCK: usize = 256;
pub(crate) const CAPTURE_CAPACITY: usize = 65536;

/// Asynchronous capture clocks are reconciled with a bounded reservoir. Ratio
/// correction responds to fill error, capped at +/-1%; linear interpolation
/// needs two lookahead frames and preserves exact samples at ratio one.
pub(crate) struct InputResampler {
    queue: Arc<AtomicQueue>,
    enabled: bool,
    prepared_frames: usize,
    primed: bool,
    purging: bool,
    current: [f32; 2],
    next: [f32; 2],
    phase: f64,
    ratio: f64,
}

impl InputResampler {
    pub(crate) fn new(queue: Arc<AtomicQueue>, enabled: bool, prepared_frames: usize) -> Self {
        Self {
            queue,
            enabled,
            prepared_frames,
            primed: false,
            purging: false,
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
        if self.queue.len() > target.saturating_mul(4) {
            self.primed = false;
            self.purging = true;
        }
        if !self.primed {
            if self.purging {
                // Recover towards recent input without draining 65536 frames
                // in one callback. Further bounded progress happens per output
                // frame while this callback's external input remains silent.
                let budget = frames.saturating_mul(4).min(4096);
                for _ in 0..budget {
                    if self.queue.len() <= target {
                        break;
                    }
                    if self.queue.try_pop().is_none() {
                        break;
                    }
                }
                if self.queue.len() > target {
                    return;
                }
            }
            if self.queue.len() < target {
                return;
            }
            let Some(current) = self.queue.try_pop().map(unpack_frame) else {
                return;
            };
            let Some(next) = self.queue.try_pop().map(unpack_frame) else {
                return;
            };
            self.current = current;
            self.next = next;
            self.phase = 0.0;
            self.primed = true;
            self.purging = false;
        }
        let fill = self.queue.len() + 2;
        self.ratio =
            1.0 + ((fill as f64 - target as f64) / (target as f64 * 100.0)).clamp(-0.01, 0.01);
    }
    /// The boolean marks a genuine underflow after priming, not startup silence.
    pub(crate) fn sample(&mut self) -> ([f32; 2], bool) {
        if !self.enabled || !self.primed {
            if self.enabled && self.purging {
                let _ = self.queue.try_pop();
            }
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
            let Some(next) = self.queue.try_pop().map(unpack_frame) else {
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
    queue: AtomicQueue,
    panic: AtomicBool,
    overflows: AtomicU64,
    releases: AtomicU64,
    epochs: [AtomicU32; 65],
}
pub(crate) fn pack_frame(frame: [f32; 2]) -> u64 {
    u64::from(frame[0].to_bits()) | (u64::from(frame[1].to_bits()) << 32)
}
fn unpack_frame(frame: u64) -> [f32; 2] {
    [
        f32::from_bits(frame as u32),
        f32::from_bits((frame >> 32) as u32),
    ]
}
fn pack_midi(event: MidiEvent, epoch: u32) -> u64 {
    u64::from(event.data[0] & 0x7f)
        | (u64::from(event.data[1]) << 7)
        | (u64::from(event.data[2] & 0x7f) << 14)
        | (u64::from(event.len) << 21)
        | (u64::from(event.source) << 23)
        | (u64::from(epoch) << 30)
}
fn unpack_midi(value: u64) -> (MidiEvent, u32) {
    (
        MidiEvent {
            source: ((value >> 23) & 0x7f) as u16,
            data: [
                (value as u8 & 0x7f) | 0x80,
                ((value >> 7) & 0x7f) as u8,
                ((value >> 14) & 0x7f) as u8,
            ],
            len: ((value >> 21) & 3) as u8,
        },
        (value >> 30) as u32,
    )
}

impl Default for MidiQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl MidiQueue {
    pub fn new() -> Self {
        Self {
            queue: AtomicQueue::new(MIDI_CAPACITY),
            panic: AtomicBool::new(false),
            overflows: AtomicU64::new(0),
            releases: AtomicU64::new(0),
            epochs: std::array::from_fn(|_| AtomicU32::new(0)),
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
        let queued = pack_midi(
            event,
            self.epochs[usize::from(event.source)].load(Ordering::Acquire),
        );
        if !self.queue.try_push(queued) {
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
    /// Called after the connection is closed. It invalidates already queued
    /// events and requests bounded, source-specific releases on the callback.
    pub fn release_source(&self, source: u16) {
        if !(1..=64).contains(&source) {
            return;
        }
        self.epochs[usize::from(source)].fetch_add(1, Ordering::AcqRel);
        self.releases
            .fetch_or(1u64 << (source - 1), Ordering::Release);
    }
    pub(crate) fn take_releases(&self) -> u64 {
        self.releases.swap(0, Ordering::AcqRel)
    }
    pub(crate) fn take_panic(&self) -> bool {
        if !self.panic.swap(false, Ordering::AcqRel) {
            return false;
        }
        // A producer can be preempted after encoding but before publishing.
        // Advance every source generation before draining, so that its eventual
        // old note-on cannot restart a note after this emergency release.
        for epoch in &self.epochs {
            epoch.fetch_add(1, Ordering::AcqRel);
        }
        // A fixed upper bound even while producers continue; stale queued note-ons
        // must not follow an emergency release.
        for _ in 0..MIDI_CAPACITY {
            if self.queue.try_pop().is_none() {
                break;
            }
        }
        true
    }
    pub(crate) fn pop_raw(&self) -> Option<(MidiEvent, bool)> {
        self.queue.try_pop().map(|queued| {
            let (event, epoch) = unpack_midi(queued);
            let valid = epoch == self.epochs[usize::from(event.source)].load(Ordering::Acquire);
            (event, valid)
        })
    }
    #[cfg(test)]
    fn pop(&self) -> Option<MidiEvent> {
        self.pop_raw()
            .and_then(|(event, valid)| valid.then_some(event))
    }
}

/// Devices share ownership within one MIDI channel/pitch. Channels remain
/// separate, and the final release retains the first emitted source identity.
pub(crate) struct MidiRouter {
    held: Vec<bool>,
    counts: Vec<u16>,
    output_source: Vec<u16>,
    channel: Option<u8>,
    reset_scope: MidiResetScope,
    release_cursor: [usize; 65],
}

impl MidiRouter {
    #[cfg(test)]
    pub(crate) fn new(channel: Option<u8>) -> Self {
        Self::with_scope(channel, MidiResetScope::Channel)
    }
    pub(crate) fn with_scope(channel: Option<u8>, reset_scope: MidiResetScope) -> Self {
        Self {
            held: vec![false; 65 * 16 * 128],
            counts: vec![0; 16 * 128],
            output_source: vec![0; 16 * 128],
            channel,
            reset_scope,
            release_cursor: [0; 65],
        }
    }
    pub(crate) fn clear(&mut self) {
        self.held.fill(false);
        self.counts.fill(0);
    }
    pub(crate) fn route_into(&mut self, event: MidiEvent, output: &mut [MidiEvent]) -> usize {
        let channel = event.data[0] & 15;
        if event.source != 0 && self.channel.is_some_and(|selected| selected != channel + 1) {
            return 0;
        }
        let kind = event.data[0] & 0xf0;
        if kind == 0xb0 && matches!(event.data[1], 120 | 123) {
            if self.reset_scope == MidiResetScope::Global {
                if output.is_empty() {
                    return 0;
                }
                self.clear();
                output[0] = event;
                return 1;
            }
            // Maximum 128 releases; callers reserve this space before dequeue.
            if output.len() < 128 {
                return 0;
            }
            let mut emitted = 0;
            for note in 0..128 {
                let owner = (usize::from(event.source) * 16 + usize::from(channel)) * 128 + note;
                let key = usize::from(channel) * 128 + note;
                if !self.held[owner] {
                    continue;
                }
                self.held[owner] = false;
                self.counts[key] -= 1;
                if self.counts[key] == 0 {
                    output[emitted] = MidiEvent {
                        source: self.output_source[key],
                        data: [0x80 | channel, note as u8, 0],
                        len: 3,
                    };
                    emitted += 1;
                }
            }
            return emitted;
        }
        if output.is_empty() {
            return 0;
        }
        if let Some(event) = self.route_note(event) {
            output[0] = event;
            1
        } else {
            0
        }
    }
    pub(crate) fn release_source_into(
        &mut self,
        source: u16,
        output: &mut [MidiEvent],
    ) -> (usize, bool) {
        let index = usize::from(source);
        let mut emitted = 0;
        // At most 2048 ownership probes per source/block, with at most the
        // caller's fixed event capacity emitted. Resume a large chord next block.
        while self.release_cursor[index] < 16 * 128 && emitted < output.len() {
            let key = self.release_cursor[index];
            self.release_cursor[index] += 1;
            let owner = index * 16 * 128 + key;
            if !self.held[owner] {
                continue;
            }
            self.held[owner] = false;
            self.counts[key] -= 1;
            if self.counts[key] == 0 {
                output[emitted] = MidiEvent {
                    source: self.output_source[key],
                    data: [0x80 | (key / 128) as u8, (key % 128) as u8, 0],
                    len: 3,
                };
                emitted += 1;
            }
        }
        let done = self.release_cursor[index] == 16 * 128;
        if done {
            self.release_cursor[index] = 0;
        }
        (emitted, done)
    }
    fn route_note(&mut self, mut event: MidiEvent) -> Option<MidiEvent> {
        let channel = event.data[0] & 15;
        let kind = event.data[0] & 0xf0;
        if kind != 0x80 && kind != 0x90 {
            return Some(event);
        }
        if event.len != 3 {
            return None;
        }
        let note = usize::from(event.data[1]);
        let key = usize::from(channel) * 128 + note;
        let owner = (usize::from(event.source) * 16 + usize::from(channel)) * 128 + note;
        let on = kind == 0x90 && event.data[2] != 0;
        if on {
            if self.held[owner] {
                return None;
            }
            self.held[owner] = true;
            self.counts[key] += 1;
            if self.counts[key] > 1 {
                return None;
            }
            self.output_source[key] = event.source;
        } else {
            if !self.held[owner] {
                return None;
            }
            self.held[owner] = false;
            self.counts[key] -= 1;
            if self.counts[key] != 0 {
                return None;
            }
            event.data[0] = 0x80 | channel;
            event.source = self.output_source[key];
        }
        Some(event)
    }
    #[cfg(test)]
    fn route(&mut self, event: MidiEvent) -> Option<MidiEvent> {
        let mut output = [event; 128];
        let count = self.route_into(event, &mut output);
        (count != 0).then_some(output[0])
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
        assert!(r.route(event(0, 1, true)).is_some());
        assert!(r.route(event(1, 0, false)).is_none());
        assert_eq!(r.route(event(2, 0, false)), Some(event(1, 0, false)));
        assert!(r.route(event(0, 1, false)).is_some());
    }
    #[test]
    fn controller_release_isolated_to_source_channel_and_global_policy_is_explicit() {
        for controller in [120, 123] {
            let mut router = MidiRouter::new(None);
            assert!(router.route(event(1, 0, true)).is_some());
            assert!(router.route(event(2, 0, true)).is_none());
            assert!(router.route(event(2, 1, true)).is_some());
            let reset = MidiEvent {
                source: 2,
                data: [0xb0, controller, 0],
                len: 3,
            };
            let mut output = [reset; 128];
            assert_eq!(router.route_into(reset, &mut output), 0);
            assert_eq!(router.route(event(1, 0, false)), Some(event(1, 0, false)));
            assert_eq!(router.route(event(2, 1, false)), Some(event(2, 1, false)));
            let mut global = MidiRouter::with_scope(None, MidiResetScope::Global);
            global.route(event(1, 0, true));
            global.route(event(2, 1, true));
            assert_eq!(global.route(reset), Some(reset));
            assert!(global.route(event(1, 0, false)).is_none());
            assert!(global.route(event(2, 1, false)).is_none());
        }
    }
    #[test]
    fn unplug_releases_only_that_device_and_invalidates_queued_events() {
        let queue = MidiQueue::new();
        let mut router = MidiRouter::with_scope(None, MidiResetScope::Global);
        router.route(event(1, 0, true));
        router.route(event(2, 0, true));
        router.route(event(3, 1, true));
        assert!(queue.push(event(1, 0, true)));
        queue.release_source(1);
        assert_eq!(queue.take_releases(), 1);
        assert!(!queue.pop_raw().expect("old event").1);
        let mut output = [event(1, 0, false); 256];
        assert_eq!(router.release_source_into(1, &mut output), (0, true));
        assert_eq!(router.route(event(2, 0, false)), Some(event(1, 0, false)));
        assert_eq!(router.route(event(3, 1, false)), Some(event(3, 1, false)));
    }
    #[test]
    fn large_unplug_release_is_bounded_and_resumes_next_block() {
        let mut router = MidiRouter::new(None);
        for channel in 0..16 {
            for note in 0..128 {
                router.route(MidiEvent {
                    source: 1,
                    data: [0x90 | channel, note, 100],
                    len: 3,
                });
            }
        }
        let mut output = [event(1, 0, false); 256];
        let mut total = 0;
        for block in 0..8 {
            let (count, done) = router.release_source_into(1, &mut output);
            assert_eq!(count, 256);
            total += count;
            assert_eq!(done, block == 7);
        }
        assert_eq!(total, 2048);
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
    fn resumed_preempted_producer_cannot_publish_a_stale_note_after_panic() {
        let queue = MidiQueue::new();
        let delayed = pack_midi(event(1, 0, true), queue.epochs[1].load(Ordering::Acquire));
        queue.panic();
        assert!(queue.take_panic());
        assert!(
            queue.queue.try_push(delayed),
            "producer resumes publication"
        );
        assert!(
            !queue.pop_raw().expect("published packet").1,
            "old encoded generation is rejected"
        );
        assert!(queue.push(event(1, 0, true)));
        assert!(queue.pop_raw().expect("fresh packet").1);
    }
    #[test]
    fn packed_midi_preserves_full_source_epoch_and_message_bits() {
        for source in [0, 1, 64] {
            for epoch in [0, 1, u32::MAX] {
                for status in [0x80, 0xbf, 0xcf, 0xdf, 0xef] {
                    let event = MidiEvent {
                        source,
                        data: [status, 127, 126],
                        len: if matches!(status & 0xf0, 0xc0 | 0xd0) {
                            2
                        } else {
                            3
                        },
                    };
                    assert_eq!(unpack_midi(pack_midi(event, epoch)), (event, epoch));
                }
            }
        }
    }
    #[test]
    fn concurrent_producers_do_not_overwrite_each_other() {
        let queue = std::sync::Arc::new(MidiQueue::new());
        let accepted = std::sync::atomic::AtomicUsize::new(0);
        std::thread::scope(|scope| {
            for source in 1..=4 {
                let q = std::sync::Arc::clone(&queue);
                let accepted = &accepted;
                scope.spawn(move || {
                    for _ in 0..512 {
                        if q.push(event(source, 0, true)) {
                            accepted.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                });
            }
        });
        let mut counts = [0; 4];
        for _ in 0..accepted.load(Ordering::Relaxed) {
            let event = queue.pop().expect("queued");
            counts[usize::from(event.source) - 1] += 1;
        }
        assert_eq!(
            counts.iter().sum::<usize>(),
            accepted.load(Ordering::Relaxed)
        );
        assert!(counts.iter().all(|count| *count <= 512));
        assert_eq!(
            queue.overflows() as usize + accepted.load(Ordering::Relaxed),
            MIDI_CAPACITY
        );
        assert!(queue.pop().is_none());
        assert_eq!(
            queue.take_panic(),
            queue.overflows() != 0,
            "a contended producer fails closed"
        );
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
                let queue = Arc::new(AtomicQueue::new(CAPTURE_CAPACITY));
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
                            queue.try_push(pack_frame([0.5, -0.5])),
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
        let queue = Arc::new(AtomicQueue::new(64));
        let mut input = InputResampler::new(Arc::clone(&queue), true, 8);
        for sample in 0..16 {
            assert!(queue.try_push(pack_frame([sample as f32, -(sample as f32)])));
        }
        input.begin(8);
        for sample in 0..8 {
            assert_eq!(input.sample(), ([sample as f32, -(sample as f32)], false));
        }
        for sample in 16..24 {
            assert!(queue.try_push(pack_frame([sample as f32, -(sample as f32)])));
        }
        input.begin(8);
        for sample in 8..16 {
            assert_eq!(input.sample(), ([sample as f32, -(sample as f32)], false));
        }
        let mut disabled = InputResampler::new(queue, false, 8);
        disabled.begin(8);
        assert_eq!(disabled.sample(), ([0.0; 2], false));
    }
    #[test]
    fn delayed_start_and_overflow_discard_stale_capture_with_bounded_callback_work() {
        let queue = Arc::new(AtomicQueue::new(CAPTURE_CAPACITY));
        for sample in 1..=48000 {
            assert!(queue.try_push(pack_frame([sample as f32, 0.0])));
        }
        let mut input = InputResampler::new(Arc::clone(&queue), true, 128);
        let mut latest = 48000;
        let mut heard = None;
        for _ in 0..200 {
            for _ in 0..128 {
                latest += 1;
                queue.force_push(pack_frame([latest as f32, 0.0]));
            }
            let before = queue.len();
            input.begin(128);
            assert!(before - queue.len() <= 514, "bounded prime purge");
            for _ in 0..128 {
                let (sample, underflow) = input.sample();
                assert!(!underflow);
                if sample[0] != 0.0 && heard.is_none() {
                    heard = Some((sample[0], latest));
                }
            }
            if heard.is_some() {
                break;
            }
        }
        let (sample, latest) = heard.expect("recent capture resumes");
        assert!(
            (latest as f32 - sample) < 512.0,
            "old audio must not play first"
        );
        for sample in 0..CAPTURE_CAPACITY + 100 {
            queue.force_push(pack_frame([sample as f32, 0.0]));
        }
        input.begin(128);
        assert_eq!(
            input.sample().0,
            [0.0; 2],
            "overflow recovery is silent until recent frames are primed"
        );
    }
}
