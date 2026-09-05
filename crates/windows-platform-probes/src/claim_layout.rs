// Copyright (c) Mike Grier.

//! Three apportionments of the reserving claim word, for measurement.
//!
//! **An experiment, not a component.** These are deliberately duplicated
//! implementations of `windows-waitable-queues`' `reserving_mpsc` claim
//! protocol, built here so the shipping crate is not disturbed while the
//! layouts are compared. See CHECKLIST-claim-word-layout.md; the
//! merge-or-delete decision is `CW-1.6`.
//!
//! The protocol is the shipping one: producers claim a position by advancing a
//! packed `(reserved, position)` word with one compare-and-swap, then wait to
//! observe a `head` that has passed the slot's previous occupant before
//! writing it. Only the word's width and split differ between the three.
//!
//! | Layout | Word | reserved / position | Recurrence at |
//! |---|---|---|---|
//! | [`narrow`] | `u64` | 32 / 32 | 2^32 pushes |
//! | [`deep`] | `u64` | 16 / 48 | 2^48 pushes |
//! | [`wide`] | `u128` | 64 / 64 | 2^64 pushes |
//!
//! **`deep` decouples the reservation ceiling from the capacity.** The shipping
//! shape requires the `reserved` half to hold the entire capacity, because
//! every slot may be reserved at once; that is what makes a 2^31 capacity
//! ceiling consume 32 bits. Capping *outstanding reservations* at 65535 while
//! leaving the capacity bounded only by the ring lets the position keep 48
//! bits. Reservations exist for messages that must not be lost, so a ceiling
//! far below the capacity is a different promise rather than a broken one --
//! but it is a contract change, which is why it is measured before it is
//! proposed.
//!
//! Hand-written three times rather than made generic over a layout trait, for
//! the reason `time_isolated_permit` is a line-for-line twin of its neighbour:
//! an abstraction that might not inline identically would be reported as the
//! algorithm's cost, in a measurement whose whole output is a difference of a
//! few nanoseconds per push.
//!
//! Items are `u64` throughout. That keeps the slot payload identical across the
//! three so the comparison is of claim words and slot metadata, and it removes
//! drop glue from the timed region.

#[cfg(test)]
mod tests;

/// Isolates a field onto its own cache line.
///
/// **Load-bearing, and measured to be.** The shipping shape puts both `head`
/// and the claim word behind this, because every producer reads `head` on
/// every push and the consumer writes it; sharing a line puts the consumer's
/// writes directly in every producer's path. A first version of this module
/// omitted the padding and measured 193.8 ns/push against the shipping shape's
/// 51.8 at 32 producers -- a 3.7x gap that was the missing alignment, not the
/// layouts being compared. 128 rather than 64 to match, which is what the
/// prefetcher pulling an adjacent line makes necessary.
#[repr(align(128))]
struct CacheAligned<T>(T);

pub mod narrow {
    //! The shipping apportionment: a `u64` word split 32 / 32.

    use std::cell::UnsafeCell;
    use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

    /// Bits of the claim word given to the position.
    const POSITION_BITS: u32 = 32;

    /// Isolates the position half of the claim word.
    const POSITION_MASK: u64 = (1 << POSITION_BITS) - 1;

    /// One cell of the ring.
    struct Slot {
        /// `position + 1` once the claiming producer has finished writing.
        sequence: AtomicU32,
        value: UnsafeCell<u64>,
    }

    /// A bounded MPSC whose claim word is split 32 / 32.
    pub struct Queue {
        claim: super::CacheAligned<AtomicU64>,
        head: super::CacheAligned<AtomicU32>,
        mask: u32,
        capacity: u32,
        slots: Box<[Slot]>,
    }

    // SAFETY: a position is claimed by exactly one producer, which is therefore
    // the slot's only writer, and it publishes with a release store the
    // consumer acquires. The consumer is the only reader and advances `head`
    // with a release store the producers acquire before reusing the slot.
    unsafe impl Sync for Queue {}
    // SAFETY: as above; the payload is `u64`, which is `Send`.
    unsafe impl Send for Queue {}

    impl Queue {
        /// Build a queue whose capacity is `capacity`, which must be a power of two.
        #[must_use]
        pub fn with_capacity(capacity: usize) -> Self {
            assert!(
                capacity.is_power_of_two(),
                "capacity must be a power of two"
            );
            let slots = (0..capacity)
                .map(|_| Slot {
                    sequence: AtomicU32::new(0),
                    value: UnsafeCell::new(0),
                })
                .collect::<Vec<_>>()
                .into_boxed_slice();
            Self {
                claim: super::CacheAligned(AtomicU64::new(0)),
                head: super::CacheAligned(AtomicU32::new(0)),
                mask: (capacity - 1) as u32,
                capacity: capacity as u32,
                slots,
            }
        }

        /// Claim a position and publish `item`, or report the queue full.
        pub fn push(&self, item: u64) -> bool {
            let mut word = self.claim.0.load(Ordering::Relaxed);
            let position = loop {
                let position = (word & POSITION_MASK) as u32;
                let reserved = (word >> POSITION_BITS) as u32;
                let occupied = position.wrapping_sub(self.head.0.load(Ordering::Acquire));
                if occupied >= self.capacity - reserved {
                    // Provisional: `position` and `head` were read at different
                    // instants, so re-read the claim before believing it.
                    let current = self.claim.0.load(Ordering::Relaxed);
                    if current != word {
                        word = current;
                        continue;
                    }
                    return false;
                }
                let next = ((reserved as u64) << POSITION_BITS) | position.wrapping_add(1) as u64;
                match self.claim.0.compare_exchange_weak(
                    word,
                    next,
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => break position,
                    Err(actual) => word = actual,
                }
            };

            // The acquire edge the slot write needs: the consumer frees a slot
            // with a release store to `head`, and this must observe one that has
            // passed this position's previous occupant.
            while position.wrapping_sub(self.head.0.load(Ordering::Acquire)) >= self.capacity {
                std::hint::spin_loop();
            }

            let slot = &self.slots[(position & self.mask) as usize];
            // SAFETY: this thread claimed `position`, so it is the only writer,
            // and the loop above established the previous occupant is gone.
            unsafe { *slot.value.get() = item };
            slot.sequence
                .store(position.wrapping_add(1), Ordering::Release);
            true
        }

        /// Take the oldest published item. Single consumer only.
        pub fn pop(&self) -> Option<u64> {
            let head = self.head.0.load(Ordering::Relaxed);
            let slot = &self.slots[(head & self.mask) as usize];
            if slot.sequence.load(Ordering::Acquire) != head.wrapping_add(1) {
                return None;
            }
            // SAFETY: the sequence read above synchronizes-with the producer's
            // release store, so the write of this item happens-before this read.
            let item = unsafe { *slot.value.get() };
            self.head.0.store(head.wrapping_add(1), Ordering::Release);
            Some(item)
        }
    }
}

pub mod deep {
    //! An asymmetric apportionment: a `u64` word split 16 / 48.

    use std::cell::UnsafeCell;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// Bits of the claim word given to the position.
    const POSITION_BITS: u32 = 48;

    /// Isolates the position half of the claim word.
    const POSITION_MASK: u64 = (1 << POSITION_BITS) - 1;

    /// One cell of the ring.
    struct Slot {
        /// Widened to match the position: a sequence narrower than the position
        /// would alias every 2^32 and reintroduce the recurrence on the
        /// consumer's side, which is the defect the split exists to remove.
        sequence: AtomicU64,
        value: UnsafeCell<u64>,
    }

    /// A bounded MPSC whose claim word is split 16 / 48.
    pub struct Queue {
        claim: super::CacheAligned<AtomicU64>,
        head: super::CacheAligned<AtomicU64>,
        mask: u64,
        capacity: u64,
        slots: Box<[Slot]>,
    }

    // SAFETY: as `narrow`'s; the protocol is identical and only the split differs.
    unsafe impl Sync for Queue {}
    // SAFETY: as above.
    unsafe impl Send for Queue {}

    impl Queue {
        /// Build a queue whose capacity is `capacity`, which must be a power of two.
        #[must_use]
        pub fn with_capacity(capacity: usize) -> Self {
            assert!(
                capacity.is_power_of_two(),
                "capacity must be a power of two"
            );
            let slots = (0..capacity)
                .map(|_| Slot {
                    sequence: AtomicU64::new(u64::MAX),
                    value: UnsafeCell::new(0),
                })
                .collect::<Vec<_>>()
                .into_boxed_slice();
            Self {
                claim: super::CacheAligned(AtomicU64::new(0)),
                head: super::CacheAligned(AtomicU64::new(0)),
                mask: (capacity - 1) as u64,
                capacity: capacity as u64,
                slots,
            }
        }

        /// Claim a position and publish `item`, or report the queue full.
        pub fn push(&self, item: u64) -> bool {
            let mut word = self.claim.0.load(Ordering::Relaxed);
            let position = loop {
                let position = word & POSITION_MASK;
                let reserved = word >> POSITION_BITS;
                // Masked because the position wraps at 2^48 rather than at the
                // word's own width, which is the cost an asymmetric split pays
                // and a 32 / 32 one gets free from `u32` truncation.
                let occupied =
                    position.wrapping_sub(self.head.0.load(Ordering::Acquire)) & POSITION_MASK;
                if occupied >= self.capacity - reserved {
                    let current = self.claim.0.load(Ordering::Relaxed);
                    if current != word {
                        word = current;
                        continue;
                    }
                    return false;
                }
                let next = (reserved << POSITION_BITS) | (position.wrapping_add(1) & POSITION_MASK);
                match self.claim.0.compare_exchange_weak(
                    word,
                    next,
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => break position,
                    Err(actual) => word = actual,
                }
            };

            while (position.wrapping_sub(self.head.0.load(Ordering::Acquire)) & POSITION_MASK)
                >= self.capacity
            {
                std::hint::spin_loop();
            }

            let slot = &self.slots[(position & self.mask) as usize];
            // SAFETY: as `narrow`'s -- sole claimant, previous occupant gone.
            unsafe { *slot.value.get() = item };
            slot.sequence
                .store(position.wrapping_add(1) & POSITION_MASK, Ordering::Release);
            true
        }

        /// Take the oldest published item. Single consumer only.
        pub fn pop(&self) -> Option<u64> {
            let head = self.head.0.load(Ordering::Relaxed);
            let slot = &self.slots[(head & self.mask) as usize];
            if slot.sequence.load(Ordering::Acquire) != (head.wrapping_add(1) & POSITION_MASK) {
                return None;
            }
            // SAFETY: as `narrow`'s.
            let item = unsafe { *slot.value.get() };
            self.head
                .0
                .store(head.wrapping_add(1) & POSITION_MASK, Ordering::Release);
            Some(item)
        }
    }
}

pub mod wide {
    //! The double-width apportionment: a `u128` word split 64 / 64.

    use portable_atomic::AtomicU128;
    use std::cell::UnsafeCell;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// Bits of the claim word given to the position.
    const POSITION_BITS: u32 = 64;

    /// One cell of the ring.
    struct Slot {
        sequence: AtomicU64,
        value: UnsafeCell<u64>,
    }

    /// A bounded MPSC whose claim word is a `u128` split 64 / 64.
    pub struct Queue {
        claim: super::CacheAligned<AtomicU128>,
        head: super::CacheAligned<AtomicU64>,
        mask: u64,
        capacity: u64,
        slots: Box<[Slot]>,
    }

    // SAFETY: as `narrow`'s; the protocol is identical and only the width differs.
    unsafe impl Sync for Queue {}
    // SAFETY: as above.
    unsafe impl Send for Queue {}

    impl Queue {
        /// Build a queue whose capacity is `capacity`, which must be a power of two.
        #[must_use]
        pub fn with_capacity(capacity: usize) -> Self {
            assert!(
                capacity.is_power_of_two(),
                "capacity must be a power of two"
            );
            let slots = (0..capacity)
                .map(|_| Slot {
                    sequence: AtomicU64::new(u64::MAX),
                    value: UnsafeCell::new(0),
                })
                .collect::<Vec<_>>()
                .into_boxed_slice();
            Self {
                claim: super::CacheAligned(AtomicU128::new(0)),
                head: super::CacheAligned(AtomicU64::new(0)),
                mask: (capacity - 1) as u64,
                capacity: capacity as u64,
                slots,
            }
        }

        /// Claim a position and publish `item`, or report the queue full.
        pub fn push(&self, item: u64) -> bool {
            let mut word = self.claim.0.load(Ordering::Relaxed);
            let position = loop {
                let position = word as u64;
                let reserved = (word >> POSITION_BITS) as u64;
                let occupied = position.wrapping_sub(self.head.0.load(Ordering::Acquire));
                if occupied >= self.capacity - reserved {
                    let current = self.claim.0.load(Ordering::Relaxed);
                    if current != word {
                        word = current;
                        continue;
                    }
                    return false;
                }
                let next = ((reserved as u128) << POSITION_BITS) | position.wrapping_add(1) as u128;
                match self.claim.0.compare_exchange_weak(
                    word,
                    next,
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => break position,
                    Err(actual) => word = actual,
                }
            };

            while position.wrapping_sub(self.head.0.load(Ordering::Acquire)) >= self.capacity {
                std::hint::spin_loop();
            }

            let slot = &self.slots[(position & self.mask) as usize];
            // SAFETY: as `narrow`'s -- sole claimant, previous occupant gone.
            unsafe { *slot.value.get() = item };
            slot.sequence
                .store(position.wrapping_add(1), Ordering::Release);
            true
        }

        /// Take the oldest published item. Single consumer only.
        pub fn pop(&self) -> Option<u64> {
            let head = self.head.0.load(Ordering::Relaxed);
            let slot = &self.slots[(head & self.mask) as usize];
            if slot.sequence.load(Ordering::Acquire) != head.wrapping_add(1) {
                return None;
            }
            // SAFETY: as `narrow`'s.
            let item = unsafe { *slot.value.get() };
            self.head.0.store(head.wrapping_add(1), Ordering::Release);
            Some(item)
        }
    }
}
