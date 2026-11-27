//! A preallocated, lock-free, single-producer single-consumer ring buffer.
//!
//! Core Audio calls an IOProc on a real-time thread, which must never
//! allocate, lock, or wait. The IOProc is the only producer: it copies a
//! callback's frames in with [`Producer::push_with`], which either writes all
//! of them or none and counts the miss. The capture thread is the only
//! consumer. When the consumer falls behind, new audio is dropped and
//! counted; the producer never blocks, and nothing already queued is
//! overwritten, so what the consumer reads is always contiguous.
//!
//! Capacity is a power of two so positions wrap with a mask. `head` and
//! `tail` count items ever written and read, so `head - tail` is the fill
//! level without a separate full flag.

use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

struct Shared<T> {
    slots: Box<[UnsafeCell<T>]>,
    mask: usize,
    /// Items written so far; only the producer stores it.
    head: AtomicUsize,
    /// Items read so far; only the consumer stores it.
    tail: AtomicUsize,
    /// Pushes refused because the ring was full.
    dropped: AtomicU64,
}

// SAFETY: a slot is written only by the single producer while it lies
// between `tail` and `tail + capacity` and is not yet published by `head`,
// and read only by the single consumer after `head` (Release/Acquire)
// published it and before `tail` (Release/Acquire) hands it back. The two
// sides never touch the same slot at the same time.
unsafe impl<T: Send> Sync for Shared<T> {}
unsafe impl<T: Send> Send for Shared<T> {}

/// The writing end. Exactly one exists per ring.
pub struct Producer<T> {
    shared: Arc<Shared<T>>,
}

/// The reading end. Exactly one exists per ring.
pub struct Consumer<T> {
    shared: Arc<Shared<T>>,
}

/// Creates a ring holding at least `min_capacity` items.
pub fn ring<T: Copy + Default>(min_capacity: usize) -> (Producer<T>, Consumer<T>) {
    let capacity = min_capacity.max(2).next_power_of_two();
    let slots = (0..capacity)
        .map(|_| UnsafeCell::new(T::default()))
        .collect::<Vec<_>>()
        .into_boxed_slice();
    let shared = Arc::new(Shared {
        slots,
        mask: capacity - 1,
        head: AtomicUsize::new(0),
        tail: AtomicUsize::new(0),
        dropped: AtomicU64::new(0),
    });
    (
        Producer {
            shared: shared.clone(),
        },
        Consumer { shared },
    )
}

impl<T: Copy> Producer<T> {
    /// Writes `count` items, item `i` being `value(i)`, if they all fit.
    /// Otherwise writes nothing, counts one drop, and returns false. Never
    /// allocates, locks, or waits.
    pub fn push_with(&self, count: usize, mut value: impl FnMut(usize) -> T) -> bool {
        let shared = &*self.shared;
        let head = shared.head.load(Ordering::Relaxed);
        let tail = shared.tail.load(Ordering::Acquire);
        let free = shared.slots.len() - head.wrapping_sub(tail);
        if count > free {
            shared.dropped.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        for i in 0..count {
            let slot = &shared.slots[head.wrapping_add(i) & shared.mask];
            // SAFETY: see `Shared`; this slot is free and unpublished.
            unsafe { *slot.get() = value(i) };
        }
        shared
            .head
            .store(head.wrapping_add(count), Ordering::Release);
        true
    }

    /// Space left, in items.
    pub fn free(&self) -> usize {
        let shared = &*self.shared;
        shared.slots.len()
            - shared
                .head
                .load(Ordering::Relaxed)
                .wrapping_sub(shared.tail.load(Ordering::Acquire))
    }

    /// Counts a callback the producer chose to drop as a whole.
    pub fn count_drop(&self) {
        self.shared.dropped.fetch_add(1, Ordering::Relaxed);
    }
}

impl<T: Copy> Consumer<T> {
    /// Items waiting to be read.
    #[cfg(test)]
    pub fn len(&self) -> usize {
        let shared = &*self.shared;
        shared
            .head
            .load(Ordering::Acquire)
            .wrapping_sub(shared.tail.load(Ordering::Relaxed))
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Reads up to `out.len()` items into `out`, returning how many.
    pub fn pop_into(&self, out: &mut [T]) -> usize {
        let shared = &*self.shared;
        let tail = shared.tail.load(Ordering::Relaxed);
        let available = shared.head.load(Ordering::Acquire).wrapping_sub(tail);
        let count = available.min(out.len());
        for (i, item) in out.iter_mut().take(count).enumerate() {
            let slot = &shared.slots[tail.wrapping_add(i) & shared.mask];
            // SAFETY: see `Shared`; this slot was published by `head`.
            *item = unsafe { *slot.get() };
        }
        shared
            .tail
            .store(tail.wrapping_add(count), Ordering::Release);
        count
    }

    /// Reads one item.
    pub fn pop(&self) -> Option<T>
    where
        T: Default,
    {
        let mut one = [T::default()];
        (self.pop_into(&mut one) == 1).then_some(one[0])
    }

    /// Pushes refused because the ring was full, since it was created.
    pub fn dropped(&self) -> u64 {
        self.shared.dropped.load(Ordering::Relaxed)
    }

    #[cfg(test)]
    pub fn capacity(&self) -> usize {
        self.shared.slots.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capacity_rounds_up_to_a_power_of_two() {
        let (_, consumer) = ring::<f32>(1000);
        assert_eq!(consumer.capacity(), 1024);
    }

    #[test]
    fn items_come_out_in_order_across_the_wraparound() {
        let (producer, consumer) = ring::<u32>(8);
        let mut next = 0u32;
        let mut expected = 0u32;
        let mut out = [0u32; 3];
        // Odd push and pop sizes walk the positions around the ring many
        // times, through every alignment of the wrap.
        for round in 0..200 {
            let count = 1 + round % 5;
            if producer.push_with(count, |i| next + i as u32) {
                next += count as u32;
            }
            let read = consumer.pop_into(&mut out);
            for value in &out[..read] {
                assert_eq!(*value, expected);
                expected += 1;
            }
        }
        while let Some(value) = consumer.pop() {
            assert_eq!(value, expected);
            expected += 1;
        }
        assert_eq!(expected, next);
        assert!(next > 300, "the ring wrapped many times");
    }

    #[test]
    fn a_push_that_does_not_fit_is_dropped_whole_and_counted() {
        let (producer, consumer) = ring::<u8>(4);
        assert!(producer.push_with(3, |i| i as u8));
        assert!(!producer.push_with(2, |_| 9), "only one slot is free");
        assert_eq!(consumer.dropped(), 1);
        assert_eq!(consumer.len(), 3, "nothing partial was written");
        assert!(producer.push_with(1, |_| 3));
        assert!(!producer.push_with(1, |_| 9));
        assert_eq!(consumer.dropped(), 2);
        let mut out = [0u8; 8];
        assert_eq!(consumer.pop_into(&mut out), 4);
        assert_eq!(
            &out[..4],
            &[0, 1, 2, 3],
            "queued audio is never overwritten"
        );
        assert!(producer.push_with(4, |i| 10 + i as u8), "space returns");
        assert_eq!(producer.free(), 0);
    }

    #[test]
    fn producer_and_consumer_on_two_threads_lose_nothing_that_was_accepted() {
        let (producer, consumer) = ring::<u64>(64);
        let writer = std::thread::spawn(move || {
            let mut accepted = Vec::new();
            for start in (0..20_000u64).step_by(7) {
                if producer.push_with(7, |i| start + i as u64) {
                    accepted.push(start);
                }
            }
            accepted
        });
        let mut seen = Vec::new();
        let mut out = [0u64; 16];
        loop {
            let read = consumer.pop_into(&mut out);
            seen.extend_from_slice(&out[..read]);
            if read == 0 && writer.is_finished() && consumer.is_empty() {
                break;
            }
        }
        let accepted = writer.join().unwrap();
        let expected: Vec<u64> = accepted
            .iter()
            .flat_map(|start| (0..7).map(move |i| start + i))
            .collect();
        assert_eq!(seen, expected);
        assert_eq!(
            consumer.dropped() as usize,
            (20_000usize).div_ceil(7) - accepted.len()
        );
    }
}
