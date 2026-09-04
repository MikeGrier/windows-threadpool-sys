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
use core::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

use crate::CacheAligned;
use crate::capacity::{Bounds, MAX_ADMISSIBLE_CAPACITY, validate_capacity};
use crate::doorbell::Doorbell;
use crate::error::{CapacityError, PushError};
use crate::metrics::Metrics;

/// A ticket, and the slot sequence numbers compared against one.
///
/// **64 bits on every target, deliberately, rather than `usize`**, for the same
/// reason [`slotwise_mpsc`](crate::slotwise_mpsc) made the same choice: a
/// 32-bit counter laps in minutes at this crate's measured rates, and a shape
/// whose soundness depends on the target's pointer width is not one this crate
/// ships twice over.
///
/// It matters *less* here than there, and that difference is the point of the
/// shape. In `slotwise_mpsc` the position is compared against a slot sequence to
/// decide whether a slot is free, so a lap is a correctness hazard. Here the
/// ticket carries **no decision at all** -- admission was already settled by the
/// permit -- so its only job is to name a distinct slot, and it could wrap
/// harmlessly. 64 bits makes it a *dumb* `fetch_add` that nobody has to reason
/// about again: no wrap analysis, no ambiguity bound to re-derive, and a
/// difference against `head` that stays meaningful for any queue this process
/// could construct.
type Position = u64;

/// What this shape accepts as a capacity. See [`BOUNDS_MAX`].
const BOUNDS: Bounds = Bounds {
    min: 2,
    max: BOUNDS_MAX,
};

/// The largest capacity this shape accepts.
///
/// The crate-wide ceiling, matching [`slotwise_mpsc`](crate::slotwise_mpsc)
/// rather than [`reserving_mpsc`](crate::reserving_mpsc). That shape's far lower
/// 2^31 is forced by its packing -- half a word for the position, half for the
/// reservation count -- and this one has no packed word to be constrained by.
///
/// **This was 2^31 while the ticket was 32 bits**, so that a measurement against
/// `reserving_mpsc` covered the same range on both. Widening the ticket removed
/// the reason: the shapes are now measured at whatever capacity the harness
/// picks, which is well below either ceiling, and matching an artificial limit
/// would only misreport what this shape can do.
pub const BOUNDS_MAX: usize = MAX_ADMISSIBLE_CAPACITY;

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
    // The permit count starts at the capacity and never exceeds it, so the
    // capacity is what must fit in the signed count's *positive* range.
    //
    // **The transient overdraft does not constrain this**, which is worth
    // stating because an earlier version of this assertion assumed it did and
    // reserved half the range for it. The overdraft goes *negative* -- each
    // concurrent claimant subtracts one before undoing -- so it consumes the
    // range below zero, of which there is a full 2^63, against a bound of one
    // per thread in flight. It cannot meet the positive bound from below.
    assert!(
        BOUNDS.max as u64 <= i64::MAX as u64,
        "the permit count must be able to hold the whole capacity"
    );
    // `len` reads the queue's depth as `tail - head` in wrapping arithmetic, and
    // that difference is unambiguous only up to half the ticket's range. So the
    // capacity must fit below that half, not merely below `Position::MAX`.
    //
    // **Stated against the half rather than the maximum deliberately.** An
    // earlier version of this assertion compared `BOUNDS.max` to `Position::MAX`,
    // which is *tautological* on every target -- a `usize` capacity cannot exceed
    // a `u64` maximum -- and so asserted nothing at all. That is precisely the
    // trap `reserving_mpsc`'s own const block records having fallen into once.
    // This form fails if `Position` is ever narrowed to `u32`, which is the
    // change it exists to catch.
    assert!(
        (BOUNDS.max as u128) <= 1_u128 << (Position::BITS - 1),
        "the ticket must be wide enough that a wrapping depth is unambiguous at any capacity this \
         shape accepts"
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
            sequence: AtomicU64::new(index as Position),
            value: UnsafeCell::new(MaybeUninit::uninit()),
        });
    }

    let shared = Arc::new(Shared {
        metrics: Metrics::new(false),
        slots: slots.into_boxed_slice(),
        mask: capacity - 1,
        capacity,
        head: CacheAligned(AtomicU64::new(0)),
        tail: CacheAligned(AtomicU64::new(0)),
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
    sequence: AtomicU64,
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
    head: CacheAligned<AtomicU64>,
    /// The ticket dispenser. Only ever `fetch_add`.
    ///
    /// Carries no decision, so it has no predicate that a recurrence could
    /// invalidate. At [`Position`]'s width it is a *dumb* increment: it would be
    /// sound if it wrapped, and it cannot wrap, so neither property has to be
    /// argued at a call site again.
    tail: CacheAligned<AtomicU64>,
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
        //
        // Release, matching `release_permit`, even though this thread published
        // nothing and needs no edge of its own. Unlike `tail` and `head`, this
        // counter carries a real edge, and a relaxed RMW here would sit in the
        // middle of it: if this undo reads from a consumer's release and a
        // third thread's acquire then reads from this undo, that thread's
        // synchronization with the consumer rests entirely on the release
        // sequence rule. That rule holds, but it was narrowed once already
        // (C++20 dropped same-thread relaxed stores from it), and it is not a
        // thing a reader should have to reconstruct to trust a slot handoff.
        // Uniform acquire/release on this counter costs one `stlxr` over
        // `stxr` on ARM64, on the contended slow path, and removes the
        // argument entirely.
        self.permits.0.fetch_add(1, Ordering::Release);
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
    unsafe fn publish(&self, position: Position, item: T) {
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

    /// Relaxed on both, because neither `tail` nor `head` ever receives a
    /// release write: `tail` only ever moves by `fetch_add(Relaxed)`, and the
    /// consumer's `head` store is deliberately relaxed (see `pop`, where the
    /// permit is what frees the slot). An acquire load here would therefore
    /// pair with nothing and synchronize with nothing -- it would read as a
    /// guarantee this queue does not make. The real edges are `sequence`
    /// (release in `publish`, acquire in `pop`) and `permits` (release in
    /// `release_permit`, acquire in `try_take_permit`); this is a snapshot and
    /// rides on neither.
    fn len(&self) -> usize {
        let tail = self.tail.0.load(Ordering::Relaxed);
        let head = self.head.0.load(Ordering::Relaxed);
        (tail.wrapping_sub(head) as usize).min(self.capacity)
    }
}

impl<T> Drop for Shared<T> {
    fn drop(&mut self) {
        // Every handle is gone, so `&mut self` proves this is the only thread.
        // `get_mut` reads each position directly rather than atomically, which
        // is why no ordering appears here at all: there is no second thread for
        // one to order against, so the question does not arise. That is why
        // this is not a relaxed read on an otherwise acquire/release atomic --
        // it is not an atomic read. `reserving_mpsc`'s drop does the same.
        //
        // A slot whose sequence marks it published still holds an item nobody
        // took.
        let mask = self.mask;
        let head = *self.head.0.get_mut();
        let tail = *self.tail.0.get_mut();
        let mut position = head;
        while position != tail {
            let slot = &mut self.slots[position as usize & mask];
            if *slot.sequence.get_mut() == position.wrapping_add(1) {
                // SAFETY: the sequence says a producer finished writing this
                // slot and no consumer took it. Every handle is gone, so this
                // is the only reader, and each position is visited once.
                unsafe {
                    slot.value.get_mut().assume_init_drop();
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
