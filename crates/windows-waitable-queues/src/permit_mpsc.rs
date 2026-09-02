// Copyright (c) Mike Grier.

//! **Experimental.** A reserving MPSC whose admission is a permit, not a room check.
//!
//! Not a shipping shape. This exists to be measured against
//! [`reserving_mpsc`](crate::reserving_mpsc) so that SH-14.3 can be decided on
//! evidence, and it is gated behind the non-default `experimental-permit-claim`
//! feature so that nothing depends on it by accident. It is exempt from this
//! crate's semver promise and will either be merged into `reserving_mpsc` or
//! deleted; see `SH-15.6`.
//!
//! # The one thing that differs
//!
//! Everything here -- the ring, the slot sequence, the publication order, the
//! doorbell ring, the reservation semantics -- is `reserving_mpsc`'s. **Only the
//! claim protocol changes**, because that is the variable under test and
//! anything else that differed would confound the measurement.
//!
//! `reserving_mpsc` decides "there is room" by reading the consumer's `head`,
//! and then compare-exchanges a claim word that does not contain `head`. The
//! decision and the operation that acts on it are separate, which is
//! [SH-14.1](../../../CHECKLIST-ship-topology-and-queues.md): a producer stalled
//! between them resumes after the 32-bit position field has recurred, its
//! exchange succeeds against a numerically equal but generations-later value,
//! and it writes a slot whose freedom was decided long ago.
//!
//! Here the decision *is* the operation. A producer takes a permit from a count
//! of unspoken-for slots with one atomic, and that single modification both
//! decides and claims. The predicate is a function solely of the word being
//! modified, so recurrence of any *other* value cannot invalidate it -- which is
//! the criterion [D-34](../DESIGN-NOTES.md#d-34) records. The position stops
//! carrying any decision at all and becomes a ticket handed out by `fetch_add`,
//! an operation with no predicate to be wrong about. It may wrap freely.
//!
//! # Why this does not contradict D-17
//!
//! [D-17](../DESIGN-NOTES.md#d-17) packs the reservation count and the position
//! into one word, and argues that two atomics cannot be made correct with any
//! amount of fencing: a pusher reads the count then writes the position while a
//! reserver writes the count then reads the position, each missing the other,
//! and no fence forbids it. That argument is sound and it is not evaded here.
//!
//! It concludes that "two independent claimants on one resource must synchronise
//! on one location". This shape agrees and picks a *different* single location.
//! Both claimants perform the same modification on `permits`; neither reads a
//! value the other writes elsewhere and then acts on it. The hazard D-17
//! describes needs a load-then-store on one side and a store-then-load on the
//! other, and there is no such pair here.
//!
//! # What this does not change
//!
//! **It is still technically blocking**, and no rearrangement of the claim can
//! make it otherwise while items live in the ring: a producer holding ticket `p`
//! that is preempted before publishing stalls a consumer that must deliver `p`
//! in order. In-order delivery, inline storage, and non-blocking progress are
//! over-constrained together. See `SH-inf.1`.

use core::cell::{Cell, UnsafeCell};
use core::marker::PhantomData;
use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicUsize, Ordering};
use std::sync::Arc;

use crate::CacheAligned;
use crate::capacity::{Bounds, MAX_ADMISSIBLE_CAPACITY, validate_capacity};
use crate::doorbell::Doorbell;
use crate::error::{CapacityError, PushError};
use crate::metrics::Metrics;

/// What this shape accepts as a capacity. See [`BOUNDS_MAX`].
const BOUNDS: Bounds = Bounds {
    min: 2,
    max: BOUNDS_MAX,
};

/// The largest capacity this shape accepts.
///
/// The same ceiling as [`reserving_mpsc`](crate::reserving_mpsc), so the two are
/// measured over the same range rather than over ranges that happen to differ.
///
/// Note what is *not* the reason for it here. In `reserving_mpsc` the ceiling is
/// forced by the packing -- half a word for the position, half for the count.
/// This shape has no packed word and could take the crate-wide bound directly;
/// it takes the narrower one anyway so that a measurement at a given capacity is
/// a measurement of the claim protocol and not of two different capacities.
pub const BOUNDS_MAX: usize = {
    let packed = 1_usize << 31;
    if packed <= MAX_ADMISSIBLE_CAPACITY {
        packed
    } else {
        MAX_ADMISSIBLE_CAPACITY
    }
};

const _: () = {
    assert!(
        BOUNDS.max.is_power_of_two(),
        "the maximum is offered to a caller as a capacity it could use, so it must itself be one \
         this shape would accept"
    );
    assert!(
        BOUNDS.min <= BOUNDS.max,
        "a shape that accepts nothing would reject every capacity with a suggestion it would also \
         reject"
    );
    // The permit count is signed and may go transiently negative by at most one
    // per concurrent claimant (see `take_permit`), so the capacity must leave
    // room below `i64::MAX` for every thread that could be in flight. A 2^31
    // ceiling against a 2^63 counter leaves 2^32 threads of headroom, which is
    // more than the process can create.
    assert!(
        BOUNDS.max as i64 <= i64::MAX / 2,
        "the permit count must hold the whole capacity with room for transient overdraft"
    );
};

/// Creates an experimental permit-claiming MPSC queue.
///
/// `capacity` must be a power of two between two and [`BOUNDS_MAX`].
///
/// # Errors
///
/// [`CapacityError`] when the capacity is outside what this shape accepts.
pub fn bounded<T>(capacity: usize) -> Result<(Producer<T>, Consumer<T>), CapacityError> {
    validate_capacity(capacity, BOUNDS)?;

    let mut slots = Vec::with_capacity(capacity);
    for index in 0..capacity {
        slots.push(Slot {
            // Anything that is not `position + 1` for the position this slot
            // first serves. Matches `reserving_mpsc`'s initialisation exactly.
            sequence: AtomicU32::new(index as u32),
            value: UnsafeCell::new(MaybeUninit::uninit()),
        });
    }

    let shared = Arc::new(Shared {
        metrics: Metrics::new(false),
        slots: slots.into_boxed_slice(),
        mask: capacity - 1,
        capacity,
        head: CacheAligned(AtomicU32::new(0)),
        tail: CacheAligned(AtomicU32::new(0)),
        permits: CacheAligned(AtomicI64::new(capacity as i64)),
        producers: AtomicUsize::new(1),
        consumer_live: AtomicBool::new(true),
        doorbell: Doorbell::new(),
    });

    Ok((
        Producer {
            shared: Arc::clone(&shared),
            not_sync: PhantomData,
        },
        Consumer {
            shared,
            not_sync: PhantomData,
        },
    ))
}

/// One cell of the ring.
struct Slot<T> {
    /// `position + 1` once the producer holding `position` has finished writing.
    sequence: AtomicU32,
    value: UnsafeCell<MaybeUninit<T>>,
}

struct Shared<T> {
    metrics: Metrics,
    slots: Box<[Slot<T>]>,
    mask: usize,
    capacity: usize,
    /// The consumer's position.
    ///
    /// **No producer reads this**, which is the structural change: in
    /// `reserving_mpsc` every push loads it to count free slots, and that load
    /// is both the cost D-26 measured and the stale input SH-14.1 exploits.
    /// Here it is consumer-private, kept atomic only so `len` can sample it.
    head: CacheAligned<AtomicU32>,
    /// The ticket dispenser. Only ever `fetch_add`.
    ///
    /// Carries no decision, so it has no predicate that a recurrence could
    /// invalidate, and it is free to wrap.
    tail: CacheAligned<AtomicU32>,
    /// Slots not currently spoken for, as a signed count.
    ///
    /// Signed because the claim is an optimistic decrement that may overshoot;
    /// see [`Shared::take_permit`].
    permits: CacheAligned<AtomicI64>,
    producers: AtomicUsize,
    consumer_live: AtomicBool,
    doorbell: Doorbell,
}

// SAFETY: as `reserving_mpsc`'s -- a slot is written by exactly one producer,
// the one whose ticket named that position, and read by exactly one consumer
// after it observes the release store that publishes it.
unsafe impl<T: Send> Sync for Shared<T> {}
// SAFETY: as above.
unsafe impl<T: Send> Send for Shared<T> {}

impl<T> Shared<T> {
    /// Takes one permit, or reports that none was available.
    ///
    /// **This single modification both decides and claims**, which is the whole
    /// point of the shape. A permit in hand is a guarantee that a slot exists
    /// for its holder; nothing observed before the modification has to still be
    /// true afterwards, because nothing observed before the modification was
    /// used.
    ///
    /// Optimistic rather than a compare-exchange loop: the decrement is
    /// unconditional and is undone when it turns out to have overdrawn. That
    /// costs one atomic on the success path where a loop costs one *plus* a
    /// retry per losing race, and contention is the regime under test.
    ///
    /// **Signed, and that is load-bearing.** An unsigned count would wrap to a
    /// huge value on overdraw, and a concurrent producer reading it would
    /// conclude there was room and proceed -- admitting more claimants than
    /// there are slots, which is precisely the failure this shape exists to
    /// prevent. Signed, an overdraw is visibly negative to everyone: each
    /// overdrawing thread sees its own non-positive result and undoes.
    ///
    /// The count can go no lower than `-(concurrent claimants)`, since each
    /// subtracts one before undoing.
    fn take_permit(&self) -> bool {
        // Acquire: pairs with the consumer's release in `release_permit`, so a
        // slot freed there is safe to overwrite by the time this returns. RMWs
        // on one location form a release sequence, so this synchronizes with
        // every earlier release on it, not merely the latest.
        if self.permits.0.fetch_sub(1, Ordering::Acquire) > 0 {
            return true;
        }
        // Overdrawn. Put it back; a concurrent claimant that saw the negative
        // value is doing the same.
        self.permits.0.fetch_add(1, Ordering::Relaxed);
        false
    }

    /// Returns one permit, freeing the slot it stood for.
    ///
    /// Release: the consumer's read of the slot must not become visible after
    /// this, or a producer could take the permit and overwrite an item the
    /// consumer had not finished taking. This store is what frees the slot,
    /// exactly as advancing `head` is in `reserving_mpsc`.
    fn release_permit(&self) {
        self.permits.0.fetch_add(1, Ordering::Release);
    }

    /// Writes and publishes `item` at `position`.
    ///
    /// # Safety
    ///
    /// The caller must hold a permit and the ticket naming `position`, so that
    /// no other producer can write this slot and the consumer has finished with
    /// whatever it held a lap ago.
    unsafe fn publish(&self, position: u32, item: T) {
        let slot = &self.slots[position as usize & self.mask];
        // SAFETY: the caller's ticket makes this thread the only writer, and its
        // permit means the consumer has finished with the previous occupant.
        unsafe {
            (*slot.value.get()).write(item);
        }

        // Release, and this is the publication: it must come after the write.
        slot.sequence
            .store(position.wrapping_add(1), Ordering::Release);

        // After the publication, never before. Kept here rather than omitted
        // because `reserving_mpsc` rings on every publish, and a push path
        // missing it would measure faster for a reason that has nothing to do
        // with the claim protocol.
        self.doorbell.signal();
    }

    fn len(&self) -> usize {
        let tail = self.tail.0.load(Ordering::Acquire);
        let head = self.head.0.load(Ordering::Acquire);
        (tail.wrapping_sub(head) as usize).min(self.capacity)
    }
}

impl<T> Drop for Shared<T> {
    fn drop(&mut self) {
        // Every handle is gone, so the positions can be read directly. A slot
        // whose sequence marks it published still holds an item nobody took.
        let head = self.head.0.load(Ordering::Relaxed);
        let tail = self.tail.0.load(Ordering::Relaxed);
        let mut position = head;
        while position != tail {
            let slot = &self.slots[position as usize & self.mask];
            if slot.sequence.load(Ordering::Relaxed) == position.wrapping_add(1) {
                // SAFETY: the sequence says a producer finished writing this
                // slot and no consumer took it. Every handle is gone, so this
                // is the only reader, and each position is visited once.
                unsafe {
                    (*slot.value.get()).assume_init_drop();
                }
            }
            position = position.wrapping_add(1);
        }
    }
}

/// A handle that can push. Clone it for more producers.
pub struct Producer<T> {
    shared: Arc<Shared<T>>,
    not_sync: PhantomData<Cell<()>>,
}

// SAFETY: the shared state is `Sync` for `T: Send`; the handle adds nothing.
unsafe impl<T: Send> Send for Producer<T> {}

impl<T> Clone for Producer<T> {
    fn clone(&self) -> Self {
        self.shared.producers.fetch_add(1, Ordering::Relaxed);
        Self {
            shared: Arc::clone(&self.shared),
            not_sync: PhantomData,
        }
    }
}

impl<T> Drop for Producer<T> {
    fn drop(&mut self) {
        if self.shared.producers.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.shared.doorbell.signal();
        }
    }
}

impl<T> Producer<T> {
    /// Pushes an item, or hands it back.
    ///
    /// # Errors
    ///
    /// [`PushError::Full`] when no unreserved room remains, and
    /// [`PushError::Disconnected`] when the consumer is gone.
    pub fn push(&self, item: T) -> Result<(), PushError<T>> {
        if !self.shared.consumer_live.load(Ordering::Acquire) {
            return Err(PushError::Disconnected(item));
        }
        if !self.shared.take_permit() {
            // Report disconnection in preference to fullness, matching
            // `reserving_mpsc`: a full queue whose consumer is gone will never
            // drain, so telling the caller to retry would be telling it to spin
            // forever.
            if !self.shared.consumer_live.load(Ordering::Acquire) {
                return Err(PushError::Disconnected(item));
            }
            self.shared.metrics.record_refusal();
            return Err(PushError::Full(item));
        }

        // The permit is held from here until the consumer takes the item. The
        // ticket carries no decision, so a relaxed fetch-add is enough: what
        // orders the write is the permit's acquire above and the release store
        // that publishes the slot.
        let position = self.shared.tail.0.fetch_add(1, Ordering::Relaxed);

        // SAFETY: this thread holds a permit and the ticket naming `position`.
        unsafe {
            self.shared.publish(position, item);
        }
        Ok(())
    }

    /// Claims a slot now for a message sent later.
    ///
    /// Takes a permit and **no ticket**, matching `reserving_mpsc`: an
    /// outstanding reservation reduces the room available to other producers
    /// without occupying a position, so it cannot stall the consumer however
    /// long it is held.
    ///
    /// # Errors
    ///
    /// [`PushError::Full`] when no room remains.
    pub fn reserve(&self) -> Result<Reservation<T>, PushError<()>> {
        if !self.shared.take_permit() {
            self.shared.metrics.record_refusal();
            return Err(PushError::Full(()));
        }
        self.shared.producers.fetch_add(1, Ordering::Relaxed);
        Ok(Reservation {
            shared: Arc::clone(&self.shared),
            spent: false,
        })
    }

    /// How many pushes have been refused for want of room.
    #[must_use]
    pub fn refused(&self) -> u64 {
        self.shared.metrics.refused()
    }
}

/// A slot claimed in advance.
pub struct Reservation<T> {
    shared: Arc<Shared<T>>,
    spent: bool,
}

// SAFETY: as `Producer`'s.
unsafe impl<T: Send> Send for Reservation<T> {}

impl<T> Reservation<T> {
    /// Delivers the message the reservation was taken for.
    ///
    /// Cannot fail for want of room: the permit taken at `reserve` is still
    /// held, so a slot is guaranteed.
    pub fn send(mut self, item: T) {
        self.spent = true;
        let position = self.shared.tail.0.fetch_add(1, Ordering::Relaxed);
        // SAFETY: the permit taken at `reserve` is still held and this ticket
        // names a position no other producer can hold.
        unsafe {
            self.shared.publish(position, item);
        }
    }
}

impl<T> Drop for Reservation<T> {
    fn drop(&mut self) {
        if !self.spent {
            // Never redeemed: give the room back.
            self.shared.release_permit();
        }
        if self.shared.producers.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.shared.doorbell.signal();
        }
    }
}

/// The single consuming handle.
pub struct Consumer<T> {
    shared: Arc<Shared<T>>,
    not_sync: PhantomData<Cell<()>>,
}

// SAFETY: as `Producer`'s.
unsafe impl<T: Send> Send for Consumer<T> {}

impl<T> Drop for Consumer<T> {
    fn drop(&mut self) {
        self.shared.consumer_live.store(false, Ordering::Release);
    }
}

impl<T> Consumer<T> {
    /// Takes the next item, if one has been published.
    pub fn pop(&self) -> Option<T> {
        // Relaxed: this thread is the only writer of `head`.
        let position = self.shared.head.0.load(Ordering::Relaxed);
        let slot = &self.shared.slots[position as usize & self.shared.mask];
        // Acquire: pairs with the producer's release store in `publish`.
        if slot.sequence.load(Ordering::Acquire) != position.wrapping_add(1) {
            return None;
        }

        // SAFETY: the sequence says the producer holding this position finished
        // writing, and the acquire above makes that write visible. This is the
        // only consumer and the position is given up below, so the item is read
        // exactly once.
        let item = unsafe { (*slot.value.get()).assume_init_read() };

        // Relaxed is enough for `head` here, unlike `reserving_mpsc`, because no
        // producer reads it. The release that actually frees the slot is the
        // permit below.
        self.shared
            .head
            .0
            .store(position.wrapping_add(1), Ordering::Relaxed);
        self.shared.release_permit();
        Some(item)
    }

    /// The exact number of items this queue holds when full.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.shared.capacity
    }

    /// Items currently held, as a snapshot.
    #[must_use]
    pub fn len(&self) -> usize {
        self.shared.len()
    }

    /// Whether the queue holds nothing, as a snapshot.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// How many pushes have been refused for want of room.
    ///
    /// Readable from this side as well as the producer's, matching the shipping
    /// shapes: a measurement harness drops its producers before reading the
    /// count, so a producer-only accessor would be unreachable exactly when the
    /// number is wanted.
    #[must_use]
    pub fn refused(&self) -> u64 {
        self.shared.metrics.refused()
    }
}

#[cfg(test)]
mod tests;
