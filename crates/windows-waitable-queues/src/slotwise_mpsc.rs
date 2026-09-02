// Copyright (c) Mike Grier.

//! The multi-producer, single-consumer bounded array queue.
//!
//! Any number of producers, one consumer, a fixed number of slots, and no
//! allocation after construction. It is the submission direction of a two-layer
//! ring, where many threads offer work and one domain thread takes it.
//!
//! # Vyukov's sequence protocol
//!
//! The obvious multi-producer array queue -- claim an index with a
//! fetch-and-add, write the slot, and let the consumer read it -- does not
//! work, because the consumer has no way to tell a slot that has been *claimed*
//! from one that has been *written*. A producer preempted between the two
//! leaves a hole, and a consumer reading through the hole reads uninitialized
//! memory.
//!
//! The remedy is a sequence number per slot, which carries both facts at once.
//! Slot `i` starts at sequence `i`, and thereafter:
//!
//! | Slot sequence, relative to a position `pos` | Meaning |
//! |---|---|
//! | `sequence == pos` | free, and this producer may claim it |
//! | `sequence == pos + 1` | written and published; the consumer may take it |
//! | `sequence < pos` | the queue is full at this position |
//! | `sequence > pos` | another producer got here first; re-read the tail |
//!
//! A producer claims a slot by advancing the shared tail with a
//! compare-and-swap, writes the item, and *publishes* it by storing
//! `pos + 1` into the slot's sequence with a release. The consumer takes a slot
//! only when it sees exactly that value, so a claimed-but-unwritten slot is
//! invisible to it. Taking an item frees the slot by storing
//! `pos + capacity`, which is the position the next lap will claim it at.
//!
//! **Lock-free, not wait-free.** A producer that loses its compare-and-swap
//! retries, and there is no bound on how many times it may lose. What is
//! guaranteed is that some producer always makes progress, and -- the property
//! that matters for an I/O submission path -- that a producer suspended by the
//! scheduler at any point blocks nobody but the consumer's view of the items
//! behind it, and never the other producers.
//!
//! **Bounded by construction, so backpressure is free.** A full queue is a slot
//! whose sequence has not come round, which costs one load to discover. There
//! is no separate count to maintain, no allocation to fail, and no policy knob:
//! the refusal *is* the backpressure.
//!
//! # The signatures, and what this shape validates
//!
//! [`spsc`](crate::spsc) wrote its intended trait signatures into its
//! documentation before its types existed, so that a second shape could be
//! checked against them rather than the traits being retrofitted to whichever
//! spelling came first. This is that second shape, and it matches: `push` and
//! `pop` take `&self`, the handles are split, and the error type is the shared
//! one. The traits themselves therefore ship with this module -- see
//! [`crate::traits`] and [D-3](../DESIGN-NOTES.md#d-3).
//!
//! Exactly one cell of `spsc`'s auto-trait table changes, which is what "the
//! multi-producer shape relaxes exactly one cell" was written to predict:
//!
//! | | [`Clone`] | [`Send`] | [`Sync`] |
//! |---|---|---|---|
//! | [`Producer`] | **yes** | yes, if `T: Send` | no |
//! | [`Consumer`] | no | yes, if `T: Send` | no |
//!
//! Producers multiply by cloning, not by sharing: each thread owns its own
//! handle. Keeping the handle `!Sync` is not a leftover from `spsc` -- it means
//! a producer handle is never touched by two threads at once, so nothing about
//! this queue's cardinality has to be remembered rather than checked.

use core::cell::{Cell, UnsafeCell};
use core::fmt;
use core::marker::PhantomData;
use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

/// A claim position, and the slot sequence numbers that are compared against
/// one.
///
/// **64 bits on every target, deliberately, rather than `usize`.** The protocol
/// below rests on a producer's observation of a slot's sequence still being
/// true when its compare-exchange succeeds, and the exchange guards only the
/// tail. A producer suspended between the two resumes safely *because* the tail
/// cannot have returned to the value it read -- which holds only while the
/// counter cannot lap.
///
/// With `usize` it can. On a 32-bit target the counter laps after 2^32 claims,
/// which at this crate's measured rates is a matter of minutes: the stalled
/// producer then sees the same tail bits, succeeds, and writes a slot that has
/// since been refilled from the previous lap of the ring. Every other guard in
/// this shape holds -- the position really is claimed by exactly one producer;
/// what fails is the older claim that the slot was free.
///
/// 2^64 claims cannot be reached, so the lap cannot happen, and the argument is
/// restored on every target rather than only on the ones where `usize` happened
/// to be wide enough. The cost is confined to 32-bit, where the exchange becomes
/// a 64-bit one (`cmpxchg8b` on x86); on a 64-bit target this is exactly what
/// `usize` already was.
type Position = u64;
use std::io;
use std::os::windows::io::{BorrowedHandle, OwnedHandle};
use std::sync::Arc;
use std::time::Duration;

use crate::CacheAligned;
use crate::blocking::{self, Parked};
use crate::capacity::{Bounds, WRAPPING_MAX_CAPACITY, validate_capacity};
use crate::disposal::Teardown;
use crate::doorbell::Doorbell;
use crate::error::{CapacityError, PushError, RecvError, RecvTimeoutError};
use crate::metrics::Metrics;
use crate::options::Options;

/// What this shape accepts as a capacity.
///
/// **The minimum is two, and it is a property of the sequence protocol rather
/// than a taste.**
/// A slot's sequence has to distinguish three states, and it does so by
/// counting: `pos` means free, `pos + 1` means published, and the consumer
/// frees it again by storing `pos + capacity`, the position the next lap will
/// claim it at. With one slot, `capacity == 1`, so "published at `pos`" and
/// "free at `pos + 1`" are the *same number* -- a producer would read the
/// sequence of the item it just pushed, conclude the slot was free, and
/// overwrite an item the consumer had not read.
///
/// It is reported rather than worked around. The obvious workaround --
/// allocating two slots and refusing the second -- would put a load of the
/// consumer's position back on the producer's hot path, which is exactly the
/// cost this protocol exists to avoid, and it would do so for every queue in
/// order to serve a capacity of one. A caller that genuinely wants a one-item
/// handoff wants [`spsc`](crate::spsc), which represents it exactly.
///
/// The maximum is the widest any shape may be, because this one's positions are
/// full-width [`usize`] values with nothing packed beside them --
/// [`reserving_mpsc`](crate::reserving_mpsc) pays for its reservations with a
/// far lower ceiling of 2^31.
///
/// **Do not choose between the shapes on this.** That ceiling counts *slots
/// allocated at construction*, not items ever pushed, and a ring of 2^31 slots
/// is tens of gigabytes before it holds anything useful. The difference is real
/// and practically unreachable.
const BOUNDS: Bounds = Bounds {
    min: 2,
    max: WRAPPING_MAX_CAPACITY,
};

/// Creates a multi-producer, single-consumer bounded array queue.
///
/// One producer handle is returned; further producers are made by cloning it,
/// and the queue is disconnected when the last of them is dropped.
///
/// `capacity` must be a power of two of at least two, and is the exact number
/// of items the queue holds -- not a hint, and not rounded. See
/// [`CapacityError`] for why a rejection is preferred to rounding. One slot is
/// not enough for this shape because its sequence protocol distinguishes
/// "published" from "free" by counting, and at `capacity == 1` those two states
/// are the same number; [`spsc`](crate::spsc) represents a one-item handoff
/// exactly.
///
/// # Errors
///
/// Returns [`CapacityError`] if `capacity` is zero, is not a power of two, is
/// less than two, or exceeds [`usize::MAX`] / 2.
///
/// # Examples
///
/// ```
/// use windows_waitable_queues::slotwise_mpsc;
///
/// let (tx, rx) = slotwise_mpsc::bounded::<u32>(4)?;
/// let second = tx.clone();
///
/// tx.push(1).expect("a fresh queue has room");
/// second.push(2).expect("a fresh queue has room");
///
/// assert_eq!(rx.pop(), Some(1));
/// assert_eq!(rx.pop(), Some(2));
/// assert_eq!(rx.pop(), None);
/// # Ok::<(), windows_waitable_queues::CapacityError>(())
/// ```
pub fn bounded<T>(capacity: usize) -> Result<(Producer<T>, Consumer<T>), CapacityError> {
    build(capacity, Options::new())
}

/// Creates a queue with something other than the default behaviour.
///
/// Identical to [`bounded`] except for what [`Options`] asks for.
///
/// **Note which switch costs this shape something.**
/// [`Options::tracking_high_water`] makes the producer read the consumer's
/// position on every push -- the single shared line this shape's push is built
/// to avoid touching. Off, which is the default, it costs one predictable
/// branch on a field that is written once at construction.
///
/// That avoidance is what distinguishes the two multi-producer shapes, but
/// **it is not what makes either one faster**: measurement found this shape the
/// slower of the two under contention, by up to 6.4x. See the crate
/// documentation for the numbers and for how to choose.
///
/// # Errors
///
/// As [`bounded`].
pub fn bounded_with<T>(
    capacity: usize,
    options: Options<T>,
) -> Result<(Producer<T>, Consumer<T>), CapacityError> {
    build(capacity, options)
}
fn build<T>(
    capacity: usize,
    options: Options<T>,
) -> Result<(Producer<T>, Consumer<T>), CapacityError> {
    validate_capacity(capacity, BOUNDS)?;

    let mut slots = Vec::with_capacity(capacity);
    for index in 0..capacity {
        slots.push(Slot {
            sequence: AtomicU64::new(index as Position),
            value: UnsafeCell::new(MaybeUninit::uninit()),
        });
    }

    let shared = Arc::new(Shared {
        teardown: Teardown::new(options.disposal),
        metrics: Metrics::new(options.track_high_water),
        slots: slots.into_boxed_slice(),
        mask: capacity - 1,
        capacity,
        head: CacheAligned(AtomicU64::new(0)),
        tail: CacheAligned(AtomicU64::new(0)),
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

/// One cell of the ring: an item, and a sequence number that says what state
/// the cell is in.
struct Slot<T> {
    /// The state machine described in the [module documentation](self).
    ///
    /// Deliberately *inside* the slot rather than gathered into a separate
    /// array. The producer that publishes a slot and the consumer that takes it
    /// touch the sequence and the item together, so keeping them adjacent costs
    /// one cache line instead of two. Slots do share lines with their
    /// neighbours, and that is intended: the contention this shape must avoid
    /// is on the two *positions*, which are padded apart below, not on the
    /// slots, which different producers touch at different indices anyway.
    sequence: AtomicU64,
    value: UnsafeCell<MaybeUninit<T>>,
}

struct Shared<T> {
    /// What becomes of undrained items at teardown.
    ///
    /// Read only by [`Shared::drop`], which holds `&mut self`, so it needs no
    /// synchronization and costs the hot paths nothing but its space.
    teardown: Teardown<T>,
    /// The counters this queue keeps about itself. See [`crate::metrics`].
    metrics: Metrics,
    slots: Box<[Slot<T>]>,
    mask: usize,
    capacity: usize,
    /// Where the consumer will next read. Written only by the consumer.
    ///
    /// Padded onto its own cache line by [`CacheAligned`], and the padding is
    /// load-bearing rather than waste. Every successful push writes `tail` and
    /// every successful pop writes `head`. Adjacent, they would share a line,
    /// and each write would invalidate the other side's copy of a value it only
    /// ever reads -- false sharing, which turns an uncontended queue into a
    /// contended one while every individual load and store stays correct. That
    /// is a cost with no symptom other than being slow, which is exactly the
    /// kind that survives a code review.
    head: CacheAligned<AtomicU64>,
    /// The claim counter. Advanced by a compare-and-swap, by any producer.
    ///
    /// Padded for the reason given on [`Shared::head`], and it matters more
    /// here than it does in `spsc`: this line is already the contended one, and
    /// letting the consumer's writes land on it too would add the consumer to
    /// the set of threads fighting over it.
    tail: CacheAligned<AtomicU64>,
    /// How many producer handles are alive.
    ///
    /// Reaching zero is the disconnection, and it is a count rather than a flag
    /// because producers multiply by cloning. Not padded: it changes only when
    /// a handle is created or destroyed, which is not a hot path.
    producers: AtomicUsize,
    consumer_live: AtomicBool,
    /// Readiness as a waitable `HANDLE`. Costs nothing until somebody asks for
    /// the handle, so a polling consumer never allocates a kernel object.
    doorbell: Doorbell,
}

// SAFETY: a slot is written by exactly one producer -- the one whose
// compare-and-swap claimed that position -- and read by exactly one consumer,
// which reads it only after observing the release store of `pos + 1` that
// publishes it. The write of the item therefore happens-before the read, and no
// two threads ever touch the same slot's contents at the same time. `T: Send`
// is required and sufficient because an item is moved between threads and never
// referenced from both.
//
// The `teardown` field is deliberately NOT covered by that argument, because it
// cannot be: it holds a boxed FnMut, which is Send but not Sync, so this
// impl is forcing Sync onto a field that does not have it. That is sound for
// a narrower reason -- the field is unreachable through a shared reference. It
// is private, no method reads it, and the only access is from Drop, which
// holds &mut self and runs when the last handle is already gone. So no two
// threads can reach it at all, concurrently or otherwise.
unsafe impl<T: Send> Sync for Shared<T> {}
// SAFETY: as above; sending the shared state is sending the items it holds.
unsafe impl<T: Send> Send for Shared<T> {}

impl<T> Shared<T> {
    /// The slot a position addresses.
    ///
    /// The cast cannot lose anything the mask would have kept: the mask is
    /// `capacity - 1`, and a capacity fits a `usize` by construction, so every
    /// bit above the mask is discarded either way. Narrowing first and masking
    /// second is the same answer as masking first and narrowing second.
    fn slot_index(&self, position: Position) -> usize {
        (position as usize) & self.mask
    }

    /// Items currently held, as a snapshot.
    ///
    /// **Counts slots a producer has claimed but not yet finished writing.**
    /// The alternative -- counting only published items -- would need a walk of
    /// the ring, and this number exists for metrics rather than for control
    /// flow. It never under-reports, so it is safe in the direction that
    /// matters for a backpressure gauge, and it is never used to decide whether
    /// to wait: [`Consumer::arm`] asks [`Shared::has_ready_item`] instead,
    /// which is the exact question `pop` answers.
    ///
    /// **Clamped to the capacity, because the two loads are not one instant.**
    /// `tail` is read first; if the consumer then drains past the value it
    /// held, `head` overtakes it and the wrapping subtraction yields a number
    /// near [`Position::MAX`] -- a bounded queue claiming to hold more items
    /// than it has slots. Over-reporting is the safe direction for this gauge
    /// and under-reporting is not, so the skew is resolved towards "full"
    /// rather than towards zero; what the clamp removes is only the impossible
    /// value.
    ///
    /// The clamp is also what makes the narrowing cast exact: the result is at
    /// most the capacity, which is a `usize` by construction.
    fn len(&self) -> usize {
        let tail = self.tail.0.load(Ordering::Acquire);
        let head = self.head.0.load(Ordering::Acquire);
        tail.wrapping_sub(head).min(self.capacity as Position) as usize
    }

    /// Whether the consumer would find an item right now.
    ///
    /// The emptiness half of the arming protocol, and it asks precisely what
    /// [`Consumer::pop`] asks: is the slot at the head position published? A
    /// claimed-but-unpublished slot answers `false`, which is the right answer
    /// -- the consumer may safely park on it, because the producer's publishing
    /// release store is followed by a signal that will wake it. Using
    /// [`Shared::len`] here instead would answer `true` and send the consumer
    /// round a spin loop until that producer got scheduled again.
    ///
    /// The `Acquire` load is one half of the pair described on
    /// [`Doorbell::signal`](crate::doorbell::Doorbell::signal): the producer
    /// stores this sequence and then loads the doorbell state, while the
    /// consumer stores the doorbell state and then loads this sequence. The
    /// sequentially consistent fences on both sides are what stop both loads
    /// from returning stale values.
    fn has_ready_item(&self) -> bool {
        let position = self.head.0.load(Ordering::Relaxed);
        let slot = &self.slots[self.slot_index(position)];
        slot.sequence.load(Ordering::Acquire) == position.wrapping_add(1)
    }
}

impl<T> Drop for Shared<T> {
    fn drop(&mut self) {
        // Every handle is gone, so no synchronization is needed and the
        // positions can be read directly. A slot between the two positions
        // still holds an item nobody took, and tearing the queue down must
        // account for them rather than leak them.
        //
        // Each is *moved out* and handed to the teardown policy rather than
        // destroyed where it lies. For the default policy the two are the same
        // thing; for a queue whose items own handles they are not, and this is
        // the only place that sees every survivor. See `crate::disposal`.
        //
        // The sequence is consulted per slot rather than assuming every
        // position in the range holds an item. A producer cannot be mid-push
        // here -- it would have to hold a handle, and there are none -- so in
        // practice every one of them does; the check states the invariant the
        // read depends on instead of leaving it to that argument.
        let mask = self.mask;
        let head = *self.head.0.get_mut();
        let tail = *self.tail.0.get_mut();
        let mut position = head;
        while position != tail {
            let published = position.wrapping_add(1);
            let slot = &mut self.slots[(position as usize) & mask];
            if *slot.sequence.get_mut() == published {
                // SAFETY: the slot's sequence says the producer finished
                // writing it and the consumer never took it, so it holds an
                // initialized item. It is read exactly once, because `position`
                // advances every iteration and the slot is never read again.
                let item = unsafe { slot.value.get_mut().assume_init_read() };
                self.teardown.dispose(item);
            }
            position = position.wrapping_add(1);
        }
    }
}

/// A writing half of an [`slotwise_mpsc`](self) queue.
///
/// [`Clone`], and that is the only difference from `spsc`'s producer: cloning
/// is how a second producer comes into existence, and the queue is disconnected
/// when the last clone is dropped.
///
/// Not [`Sync`], so a handle is used by one thread at a time. Give each thread
/// its own clone rather than sharing one behind a reference.
pub struct Producer<T> {
    shared: Arc<Shared<T>>,
    /// Removes [`Sync`] without removing [`Send`]. A [`Cell`] is exactly that
    /// shape, and no value of it is ever created.
    not_sync: PhantomData<Cell<()>>,
}

impl<T> Producer<T> {
    /// Appends an item.
    ///
    /// # Errors
    ///
    /// [`PushError::Full`] when the queue is at capacity, which is the
    /// backpressure signal rather than a malfunction, and
    /// [`PushError::Disconnected`] when the consumer is gone. Either way the
    /// item comes back, so nothing is lost by the refusal.
    pub fn push(&self, item: T) -> Result<(), PushError<T>> {
        // Relaxed: this load only proposes a position. The compare-and-swap
        // below is what makes the claim, and it fails if the proposal was
        // stale, so a stale read costs a retry rather than correctness.
        let mut position = self.shared.tail.0.load(Ordering::Relaxed);
        loop {
            let slot = &self.shared.slots[self.shared.slot_index(position)];
            // Acquire: pairs with the consumer's release store when it frees a
            // slot, so a slot it has finished with is visible as free here.
            let sequence = slot.sequence.load(Ordering::Acquire);
            // Signed, which is why the capacity is capped at half the range:
            // both positions wrap, and only a difference smaller than half the
            // range can be told apart from its complement.
            let difference = sequence.wrapping_sub(position) as isize;

            if difference < 0 {
                // The slot has not come round: the queue is full here, and
                // because positions are claimed in order it is full outright.
                //
                // Report disconnection in preference to fullness: a full queue
                // whose consumer is gone will never drain, and telling the
                // caller to retry would be telling it to spin forever.
                if !self.shared.consumer_live.load(Ordering::Acquire) {
                    // Not counted as a refusal: this is the end of the stream,
                    // not backpressure.
                    return Err(PushError::Disconnected(item));
                }
                self.shared.metrics.record_refusal();
                return Err(PushError::Full(item));
            }
            if difference > 0 {
                // Another producer claimed this position between the load of
                // the tail and now. Re-read rather than incrementing blindly:
                // several producers may have got in.
                //
                // **A mutation run reports this branch as removable, and it is
                // right.** Reversing the comparison makes the branch dead --
                // the negative case returned above -- so a stale position falls
                // through to the exchange instead, which fails precisely
                // because the position is stale and hands back the very tail
                // this branch would have loaded. The two routes end in the same
                // place. Kept because the difference is a failed
                // read-modify-write on the one line every producer touches,
                // taken on the contended path, in exchange for a load; and
                // because saying "somebody got in" where it happens is worth
                // more than leaving it to be re-derived from an exchange that
                // fails for a reason nothing states. Measured rather than
                // argued: the mutant survives thirty runs of the two
                // many-producer tests, including the capacity-two one, without
                // losing or duplicating an item.
                position = self.shared.tail.0.load(Ordering::Relaxed);
                continue;
            }
            if !self.shared.consumer_live.load(Ordering::Acquire) {
                return Err(PushError::Disconnected(item));
            }

            // Relaxed on both sides is sufficient: this exchange orders nothing
            // but the claim itself. The item's visibility comes from the
            // release store that publishes the slot below, and the freedom to
            // write the slot comes from the acquire load above.
            match self.shared.tail.0.compare_exchange_weak(
                position,
                position.wrapping_add(1),
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(actual) => position = actual,
            }
        }

        let slot = &self.shared.slots[self.shared.slot_index(position)];
        // SAFETY: this thread's compare-and-swap claimed `position`, and a
        // position is claimed by exactly one producer. The consumer will not
        // read the slot until the release store below publishes it, and the
        // slot's sequence said it was free, so no initialized item is
        // overwritten.
        unsafe {
            (*slot.value.get()).write(item);
        }

        // **Guarded, and this branch is the whole reason high-water is a
        // switch.** This shape's producer never reads `head`, which is what
        // keeps its push off the one line every thread touches. Depth cannot be
        // known without that read, so the read is taken only when somebody
        // asked for the answer.
        //
        // Note what this property does *not* buy: measurement found this shape
        // slower than `reserving_mpsc` under contention despite it, because the
        // slot sequence a producer must read instead marches through memory
        // while other producers write it. Staying off the shared line is why
        // the two shapes are different, not why either is quick.
        //
        // Off, the cost is one predictable branch on a field written once at
        // construction, so the line is shared but read-only -- the cheap kind.
        //
        // **Before the publication below, and that placement is load-bearing.**
        // The subtraction is only non-negative while the consumer cannot have
        // passed `position`, and what holds it back is precisely that
        // `position` is not published yet. Taken afterwards, the consumer is
        // free to drain past it between the two statements, `position - head`
        // goes negative, and the wrapping turns it into a vast unsigned number
        // that `fetch_max` then keeps forever -- a peak the queue never reached
        // and could not reach. Measured before this moved: about one run in
        // thirty reported a high-water mark of `usize::MAX`.
        //
        // A stale `head` is harmless in the other direction: it can only be
        // older, which over-reports the depth by the number of items drained
        // since, and that is still bounded by the capacity.
        if self.shared.metrics.tracks_high_water() {
            let head = self.shared.head.0.load(Ordering::Acquire);
            let depth = position.wrapping_sub(head).wrapping_add(1);
            // States the invariant that the placement above buys, and states it
            // where it can fail rather than only in prose. A depth cannot
            // exceed the capacity, so anything larger is the wrapped
            // subtraction -- and without this the only witness is a
            // `high_water` assertion at the end of one test, which caught the
            // real defect about once in sixty runs. Here it fires in whichever
            // push raced, in every test that tracks high water, with the
            // offending value in hand.
            debug_assert!(
                depth <= self.shared.capacity as Position,
                "depth {depth} exceeds capacity {}: the head was read after the \
                 publication and the consumer drained past this position",
                self.shared.capacity
            );
            // The assertion above is a `debug_assert`, so the cast must be
            // sound in release too. Saturating rather than `as`: a wrapped
            // subtraction on a 32-bit target would otherwise truncate to an
            // arbitrary small number and record a *lower* depth than the true
            // one, which is the direction this gauge must never err in.
            self.shared
                .metrics
                .record_depth(usize::try_from(depth).unwrap_or(usize::MAX));
        }

        // Release, and this is the publication: it must come after the write,
        // and this is what forbids the compiler and the processor from moving
        // it earlier. Until it lands, the consumer sees the slot as
        // claimed-but-empty and skips it.
        slot.sequence
            .store(position.wrapping_add(1), Ordering::Release);

        // After the publication, never before: the doorbell says "there is
        // something to take", and that must not become true before the item is
        // actually takeable. A consumer woken early would find nothing, clear
        // the doorbell, and go back to sleep on an item that is about to exist
        // -- a lost wakeup manufactured by signalling too eagerly.
        //
        // Note that a producer may signal while an *earlier* position is still
        // unpublished, so the consumer wakes and finds nothing. That is a
        // spurious wakeup, which the protocol tolerates by construction: the
        // producer holding the earlier slot signals in its turn.
        self.shared.doorbell.signal();
        Ok(())
    }

    /// The exact number of items this queue holds when full.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.shared.capacity
    }

    /// Items currently held, as a snapshot.
    ///
    /// Includes slots claimed by a producer that has not finished writing, so
    /// it never under-reports. Implemented by the internal `Shared::len`.
    #[must_use]
    pub fn len(&self) -> usize {
        self.shared.len()
    }

    /// Whether the queue holds nothing, as a snapshot.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Whether the next push would be refused for want of room, as a snapshot.
    ///
    /// Advisory only, and more advisory here than in `spsc`: another producer
    /// may take the last slot between this call and the push. Nothing is gained
    /// by testing it beforehand, since [`Self::push`] reports the same
    /// condition without the window; it is offered for metrics.
    #[must_use]
    pub fn is_full(&self) -> bool {
        self.len() >= self.shared.capacity
    }

    /// Whether the consumer has been dropped.
    #[must_use]
    pub fn is_disconnected(&self) -> bool {
        !self.shared.consumer_live.load(Ordering::Acquire)
    }
}

impl<T> Clone for Producer<T> {
    fn clone(&self) -> Self {
        // Relaxed: the thread doing the cloning already holds a live handle, so
        // the count cannot reach zero during this call and no other thread's
        // decision depends on when this increment becomes visible. The
        // `Release`/`Acquire` pairing that matters is in `Drop`, where the
        // count reaching zero publishes everything every producer pushed.
        self.shared.producers.fetch_add(1, Ordering::Relaxed);
        Self {
            shared: Arc::clone(&self.shared),
            not_sync: PhantomData,
        }
    }
}

// Hand-written rather than derived: deriving would demand `T: Debug`, which
// would make a handle to a queue of non-`Debug` items un-printable for no
// reason. The item type is not the handle's business, so the handle reports the
// queue's state instead.
impl<T> fmt::Debug for Producer<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("slotwise_mpsc::Producer")
            .field("capacity", &self.capacity())
            .field("len", &self.len())
            .field("producers", &self.shared.producers.load(Ordering::Relaxed))
            .field("disconnected", &self.is_disconnected())
            .finish()
    }
}

impl<T> Drop for Producer<T> {
    fn drop(&mut self) {
        // `AcqRel` carries both halves of what this decrement has to do. The
        // release half publishes everything this producer pushed to whichever
        // thread observes the count reaching zero, so a consumer that sees the
        // disconnection can trust that draining to empty really has drained
        // everything. The acquire half makes *this* thread -- when it is the
        // one that drives the count to zero -- see the other producers'
        // pushes, which is what makes the signal below meaningful.
        if self.shared.producers.fetch_sub(1, Ordering::AcqRel) != 1 {
            return;
        }

        // Disconnection is a wakeup like any other, and the only one nobody
        // else can deliver. A consumer blocked on the doorbell would otherwise
        // wait forever for an item that can no longer be sent -- the queue
        // would be correct and the program would still hang.
        //
        // Only the *last* producer rings: an earlier one leaving changes
        // nothing a consumer could act on, and waking it to discover that would
        // be a spurious wakeup per departing thread.
        self.shared.doorbell.signal();
    }
}

/// The reading half of an [`slotwise_mpsc`](self) queue.
///
/// Neither [`Clone`] nor [`Sync`], which is what makes "single consumer" a fact
/// the compiler checks rather than a rule to remember.
pub struct Consumer<T> {
    shared: Arc<Shared<T>>,
    /// See [`Producer::not_sync`].
    not_sync: PhantomData<Cell<()>>,
}

impl<T> Consumer<T> {
    /// Takes the oldest item, or `None` if there is none right now.
    ///
    /// `None` does not mean the queue is finished, and here it does not even
    /// mean the queue is empty: a producer may have claimed the next position
    /// and not yet published it, in which case the items behind it are not
    /// takeable either. Order is claim order, so waiting is the only correct
    /// answer -- and the producer signals the doorbell when it publishes, so
    /// waiting is not a gamble.
    ///
    /// Pair it with [`Self::is_disconnected`] to distinguish "empty for now"
    /// from "empty for good"; the order matters, and [`Self::is_disconnected`]
    /// documents which way round.
    pub fn pop(&self) -> Option<T> {
        // Relaxed: this thread is the only writer of `head`.
        let position = self.shared.head.0.load(Ordering::Relaxed);
        let slot = &self.shared.slots[self.shared.slot_index(position)];
        // Acquire: pairs with the producer's release store, so an item it
        // published is visible here.
        let sequence = slot.sequence.load(Ordering::Acquire);

        // Anything other than "published at this position" means there is
        // nothing to take: a lower sequence is a slot from the previous lap
        // that nobody has claimed yet, and a claimed-but-unpublished slot
        // carries the previous lap's sequence too.
        if sequence != position.wrapping_add(1) {
            return None;
        }

        // SAFETY: the sequence says the producer that claimed this position
        // finished writing it, and the release/acquire pair above makes that
        // write visible here. This is the only consumer, and the slot is freed
        // below, so the item is read exactly once.
        let item = unsafe { (*slot.value.get()).assume_init_read() };

        // The head moves before the slot is freed, and not after. A producer
        // that sees the freed slot may push immediately; if it did so while
        // `head` still named the old position, `len` would briefly report more
        // items than the queue can hold. Both stores are `Release`, so neither
        // may be reordered before the read of the item above, and the first may
        // not be reordered after the second.
        self.shared
            .head
            .0
            .store(position.wrapping_add(1), Ordering::Release);

        // Freeing the slot is a store of the position the *next* lap will claim
        // it at, which is one whole capacity further on. Release, because it
        // must not become visible before the item has been read out: a producer
        // that saw it early would overwrite an item this thread had not
        // finished taking.
        slot.sequence.store(
            position.wrapping_add(self.shared.capacity as Position),
            Ordering::Release,
        );
        Some(item)
    }

    /// The exact number of items this queue holds when full.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.shared.capacity
    }

    /// Items currently held, as a snapshot.
    ///
    /// Includes slots claimed by a producer that has not finished writing, so
    /// it never under-reports. Implemented by the internal `Shared::len`.
    #[must_use]
    pub fn len(&self) -> usize {
        self.shared.len()
    }

    /// Whether the queue holds nothing, as a snapshot.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Whether every producer has been dropped.
    ///
    /// **Check this only after [`Self::pop`] has returned `None`.** A producer
    /// may push and then drop, so a queue can be disconnected and still hold
    /// items; testing this first would discard them. Draining to empty and then
    /// finding the producers gone is the only order that cannot lose an item,
    /// and the release in the last producer's `Drop` is what makes every
    /// producer's preceding pushes visible to a consumer that observes it.
    #[must_use]
    pub fn is_disconnected(&self) -> bool {
        self.shared.producers.load(Ordering::Acquire) == 0
    }

    /// Borrows the queue's readiness as a waitable `HANDLE`.
    ///
    /// This is the point of the crate. The handle is a manual-reset event that
    /// is signalled while the queue has something to take, so it can go into
    /// `WaitForMultipleObjects` beside an I/O completion, a shutdown event, or
    /// a timer -- a wait that no queue with a private parking primitive can
    /// join.
    ///
    /// The event is created on the first call, so a consumer that only ever
    /// polls with [`Self::pop`] is charged for no kernel object.
    ///
    /// The borrow is deliberate: the event belongs to the queue and must not be
    /// closed. Use [`Self::doorbell_owned`] where ownership is required.
    ///
    /// # Waiting on it correctly
    ///
    /// **Do not simply wait and then drain.** Use [`Self::arm`] to decide
    /// whether waiting is safe, or the wait can miss an item and block forever:
    ///
    /// ```no_run
    /// # use windows_waitable_queues::slotwise_mpsc;
    /// # use windows_sys::Win32::System::Threading::{WaitForSingleObject, INFINITE};
    /// # use std::os::windows::io::AsRawHandle;
    /// # fn demo(rx: &slotwise_mpsc::Consumer<u32>) -> std::io::Result<()> {
    /// loop {
    ///     while let Some(item) = rx.pop() {
    ///         let _ = item;
    ///     }
    ///     if !rx.arm()? {
    ///         continue; // Something arrived; waiting now would be wrong.
    ///     }
    ///     // The end of the stream, which arming does not report. Without
    ///     // this the wait below never returns: the last producer's drop rang
    ///     // the doorbell once and `arm` has just cleared that ring.
    ///     //
    ///     // The final `pop` is not belt-and-braces -- a producer may push and
    ///     // then drop between the drain above and this check, and skipping it
    ///     // discards an item that was successfully sent.
    ///     if rx.is_disconnected() {
    ///         while let Some(item) = rx.pop() {
    ///             let _ = item;
    ///         }
    ///         return Ok(());
    ///     }
    ///     let handle = rx.doorbell()?;
    ///     // SAFETY: a live event handle borrowed for the call.
    ///     unsafe { WaitForSingleObject(handle.as_raw_handle(), INFINITE) };
    /// }
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// Returns the error from `CreateEventW` on the first call.
    pub fn doorbell(&self) -> io::Result<BorrowedHandle<'_>> {
        self.shared.doorbell.handle()
    }

    /// A duplicate of [`Self::doorbell`] that the caller owns.
    ///
    /// The duplicate names the same event, so signalling reaches both, and the
    /// caller may close its copy whenever it likes. This is the form a
    /// `ThreadpoolWait` needs, since arming one takes ownership of its target.
    ///
    /// # Errors
    ///
    /// Returns the error from `CreateEventW` or `DuplicateHandle`.
    pub fn doorbell_owned(&self) -> io::Result<OwnedHandle> {
        self.shared.doorbell.owned()
    }

    /// Clears the doorbell and reports whether a later push could be missed.
    ///
    /// `true` means the queue had nothing takeable after the doorbell was
    /// cleared, so any later push is guaranteed to signal. `false` means
    /// something arrived in the meantime: take it instead of waiting.    ///
    /// **`true` is not by itself permission to wait indefinitely.** It answers
    /// only whether a later *push* can be missed, and says nothing about the
    /// end of the stream: with every producer gone it still returns `true`,
    /// having just cleared the single ring their drop left behind. See
    /// [`Waitable::arm`](crate::Waitable::arm) for the four-step protocol an
    /// indefinite wait needs, and the example on [`Self::doorbell`] for it
    /// written out.
    ///
    /// The order inside this method is the whole correctness argument, and it
    /// is the reverse of the one that reads naturally. Checking first would
    /// leave a window in which a push both signals and has its signal erased,
    /// and the consumer would sleep on a queue that is not empty and will never
    /// be signalled again.
    ///
    /// Clearing first splits every push into two cases, and this shape's
    /// division is **not** the one `spsc` uses -- the difference is why
    /// the internal `Doorbell::clear` had to be
    /// corrected before this shape was sound:
    ///
    /// - **A push that publishes at the head before the clear** is found by the
    ///   check, so the caller does not wait.
    /// - **Every other push** -- one that publishes after the clear, and one
    ///   that publishes at a *later position* before it -- leaves the check
    ///   finding nothing, and the caller waits. That is safe because the head
    ///   position is then still owed a publication, and `clear` guarantees the
    ///   doorbell can ring again when it comes.
    ///
    /// The second case is the one that has no counterpart in `spsc`, where any
    /// push at all makes the check find something. It is why `clear` must reset
    /// the event *before* clearing the flag that mirrors it, rather than
    /// relying on this check to cover the window.
    ///
    /// This also creates the doorbell if it does not exist, which must happen
    /// before the check for the same reason: a producer running while there is
    /// no event skips signalling, so the check has to come after the event
    /// exists to catch what that skip left behind.
    ///
    /// # Errors
    ///
    /// Returns the error from `CreateEventW` on the first call.
    pub fn arm(&self) -> io::Result<bool> {
        // Before the clear, and so before the check: see above.
        self.shared.doorbell.handle()?;
        self.shared.doorbell.clear();
        #[cfg(test)]
        crate::race_hooks::ARM.run();
        // Deliberately not `is_empty`. The question is whether `pop` would find
        // something, and a slot that a producer has claimed but not published
        // is not something `pop` can find -- see `Shared::has_ready_item`.
        Ok(!self.shared.has_ready_item())
    }

    /// The last take before reporting the end of the stream.
    ///
    /// Called only after [`Self::is_disconnected`] has returned `true`, which
    /// makes the answer final rather than a snapshot: no producer remains to
    /// add anything, so `None` here means empty forever.
    ///
    /// This exists as a named step, rather than as a bare `pop` inlined into
    /// each caller, because it guards a race that is real and narrow: a
    /// producer may push *and then* drop in the window between a receive's
    /// first `pop` and its disconnection check. Reporting the disconnection
    /// without this final take would silently discard an item that was
    /// successfully sent. Being a separate function is what lets a test reach
    /// it directly instead of hoping to schedule that window.
    fn finish(&self) -> Option<T> {
        self.pop()
    }

    /// Takes the oldest item, blocking until one arrives.
    ///
    /// Parks on the doorbell rather than spinning, so a consumer with nothing
    /// to do costs nothing.
    ///
    /// # Errors
    ///
    /// [`RecvError::Disconnected`] once every producer is gone *and* the queue
    /// is drained -- items pushed before the last producer dropped are still
    /// delivered. [`RecvError::Io`] if the doorbell cannot be created or waited
    /// on.
    pub fn recv(&self) -> Result<T, RecvError> {
        blocking::recv(self)
    }

    /// Takes the oldest item, blocking until one arrives or the deadline
    /// passes.
    ///
    /// The timeout bounds the whole call, not each individual wait: a consumer
    /// woken spuriously does not get a fresh budget.
    ///
    /// # Errors
    ///
    /// [`RecvTimeoutError::Timeout`] if the deadline passes with the queue
    /// still empty, which is not a malfunction. Otherwise as [`Self::recv`].
    pub fn recv_timeout(&self, timeout: Duration) -> Result<T, RecvTimeoutError> {
        blocking::recv_timeout(self, timeout)
    }
}

impl<T> Parked for Consumer<T> {
    type Item = T;

    fn pop(&self) -> Option<T> {
        Self::pop(self)
    }

    fn finish(&self) -> Option<T> {
        Self::finish(self)
    }

    fn arm(&self) -> io::Result<bool> {
        Self::arm(self)
    }

    fn is_disconnected(&self) -> bool {
        Self::is_disconnected(self)
    }

    fn doorbell(&self) -> io::Result<BorrowedHandle<'_>> {
        Self::doorbell(self)
    }
}

/// See [`Producer`]'s impl for why this is hand-written.
impl<T> fmt::Debug for Consumer<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("slotwise_mpsc::Consumer")
            .field("capacity", &self.capacity())
            .field("len", &self.len())
            .field("producers", &self.shared.producers.load(Ordering::Relaxed))
            .field("disconnected", &self.is_disconnected())
            .finish()
    }
}

impl<T> Drop for Consumer<T> {
    fn drop(&mut self) {
        self.shared.consumer_live.store(false, Ordering::Release);
    }
}

impl<T> crate::Producer for Producer<T> {
    type Item = T;

    fn push(&self, item: T) -> Result<(), PushError<T>> {
        Self::push(self, item)
    }

    fn is_disconnected(&self) -> bool {
        Self::is_disconnected(self)
    }
}

impl<T> crate::Consumer for Consumer<T> {
    type Item = T;

    fn pop(&self) -> Option<T> {
        Self::pop(self)
    }

    fn is_disconnected(&self) -> bool {
        Self::is_disconnected(self)
    }
}

impl<T> crate::Bounded for Producer<T> {
    fn capacity(&self) -> usize {
        Self::capacity(self)
    }

    fn len(&self) -> usize {
        Self::len(self)
    }

    fn is_empty(&self) -> bool {
        Self::is_empty(self)
    }
}

impl<T> crate::Bounded for Consumer<T> {
    fn capacity(&self) -> usize {
        Self::capacity(self)
    }

    fn len(&self) -> usize {
        Self::len(self)
    }

    fn is_empty(&self) -> bool {
        Self::is_empty(self)
    }
}

impl<T> Shared<T> {
    /// The counters, as the [`Observable`](crate::Observable) trait reports
    /// them. Written once so the two handles cannot drift apart.
    fn refused(&self) -> u64 {
        self.metrics.refused()
    }

    fn doorbell_rings(&self) -> u64 {
        self.doorbell.rings()
    }

    fn high_water(&self) -> Option<usize> {
        self.metrics.high_water()
    }
}

impl<T> Producer<T> {
    /// How many pushes have been refused for want of room.
    #[must_use]
    pub fn refused(&self) -> u64 {
        self.shared.refused()
    }

    /// How many times the doorbell has actually rung.
    #[must_use]
    pub fn doorbell_rings(&self) -> u64 {
        self.shared.doorbell_rings()
    }

    /// The deepest this queue has been, if tracking was asked for.
    #[must_use]
    pub fn high_water(&self) -> Option<usize> {
        self.shared.high_water()
    }
}

impl<T> Consumer<T> {
    /// How many pushes have been refused for want of room.
    #[must_use]
    pub fn refused(&self) -> u64 {
        self.shared.refused()
    }

    /// How many times the doorbell has actually rung.
    #[must_use]
    pub fn doorbell_rings(&self) -> u64 {
        self.shared.doorbell_rings()
    }

    /// The deepest this queue has been, if tracking was asked for.
    #[must_use]
    pub fn high_water(&self) -> Option<usize> {
        self.shared.high_water()
    }
}

impl<T> crate::Observable for Producer<T> {
    fn refused(&self) -> u64 {
        Self::refused(self)
    }

    fn doorbell_rings(&self) -> u64 {
        Self::doorbell_rings(self)
    }

    fn high_water(&self) -> Option<usize> {
        Self::high_water(self)
    }
}

impl<T> crate::Observable for Consumer<T> {
    fn refused(&self) -> u64 {
        Self::refused(self)
    }

    fn doorbell_rings(&self) -> u64 {
        Self::doorbell_rings(self)
    }

    fn high_water(&self) -> Option<usize> {
        Self::high_water(self)
    }
}

impl<T> crate::Waitable for Consumer<T> {
    fn doorbell(&self) -> io::Result<BorrowedHandle<'_>> {
        Self::doorbell(self)
    }

    fn doorbell_owned(&self) -> io::Result<OwnedHandle> {
        Self::doorbell_owned(self)
    }

    fn arm(&self) -> io::Result<bool> {
        Self::arm(self)
    }
}

#[cfg(test)]
mod tests;
