//! Packed atomic queue with one-attempt producer/consumer operations. A
//! preempted producer cannot make the callback wait for a reserved slot.
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueuePush {
    Inserted,
    Replaced,
    Rejected,
}

#[derive(Debug)]
pub struct AtomicQueue {
    slots: Box<[AtomicU64]>,
    head: AtomicUsize,
    tail: AtomicUsize,
    producer: AtomicBool,
}
struct Producer<'a>(&'a AtomicBool);
impl Drop for Producer<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

impl AtomicQueue {
    pub fn new(capacity: usize) -> Self {
        assert!(
            capacity.is_power_of_two(),
            "AtomicQueue capacity must be a nonzero power of two"
        );
        Self {
            slots: (0..capacity).map(|_| AtomicU64::new(0)).collect(),
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
            producer: AtomicBool::new(false),
        }
    }
    pub fn capacity(&self) -> usize {
        self.slots.len()
    }
    pub fn len(&self) -> usize {
        let head = self.head.load(Ordering::Acquire);
        let tail = self.tail.load(Ordering::Acquire);
        tail.wrapping_sub(head).min(self.capacity())
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    fn claim(&self) -> Option<Producer<'_>> {
        self.producer
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .ok()
            .map(|_| Producer(&self.producer))
    }
    pub fn try_push(&self, value: u64) -> bool {
        self.push(value, false) != QueuePush::Rejected
    }
    /// Optionally replace the oldest published value when full. A failed gate
    /// or head compare-exchange rejects immediately; no retry loop is used.
    pub fn force_push(&self, value: u64) -> QueuePush {
        self.push(value, true)
    }
    fn push(&self, value: u64, replace: bool) -> QueuePush {
        let Some(_producer) = self.claim() else {
            return QueuePush::Rejected;
        };
        let tail = self.tail.load(Ordering::Relaxed);
        let head = self.head.load(Ordering::Acquire);
        let mut result = QueuePush::Inserted;
        if tail.wrapping_sub(head) >= self.capacity() {
            if !replace
                || self
                    .head
                    .compare_exchange(
                        head,
                        head.wrapping_add(1),
                        Ordering::AcqRel,
                        Ordering::Relaxed,
                    )
                    .is_err()
            {
                return QueuePush::Rejected;
            }
            result = QueuePush::Replaced;
        }
        self.slots[tail & (self.capacity() - 1)].store(value, Ordering::Relaxed);
        // The payload is complete before it becomes visible to the consumer.
        self.tail.store(tail.wrapping_add(1), Ordering::Release);
        result
    }
    pub fn try_pop(&self) -> Option<u64> {
        let head = self.head.load(Ordering::Acquire);
        if head == self.tail.load(Ordering::Acquire) {
            return None;
        }
        let value = self.slots[head & (self.capacity() - 1)].load(Ordering::Relaxed);
        // A replacing producer or another consumer may have advanced head. A
        // failed claim returns immediately instead of waiting for publication.
        self.head
            .compare_exchange(
                head,
                head.wrapping_add(1),
                Ordering::AcqRel,
                Ordering::Relaxed,
            )
            .ok()
            .map(|_| value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preempted_producer_does_not_publish_or_block_consumer() {
        let queue = AtomicQueue::new(8);
        let claimed = queue.claim().expect("producer claimed");
        queue.slots[0].store(99, Ordering::Relaxed);
        assert_eq!(
            queue.try_pop(),
            None,
            "unpublished data must stay invisible"
        );
        assert!(!queue.try_push(1), "contended producer returns immediately");
        queue.tail.store(1, Ordering::Release);
        drop(claimed);
        assert_eq!(queue.try_pop(), Some(99));
        assert_eq!(queue.try_pop(), None);
    }
    #[test]
    fn replacing_full_queue_keeps_fifo_and_never_duplicates() {
        let queue = AtomicQueue::new(2);
        assert!(queue.try_push(1));
        assert!(queue.try_push(2));
        assert!(!queue.try_push(3));
        assert_eq!(queue.force_push(3), QueuePush::Replaced);
        assert_eq!(queue.try_pop(), Some(2));
        assert_eq!(queue.try_pop(), Some(3));
        assert_eq!(queue.try_pop(), None);
    }
}
