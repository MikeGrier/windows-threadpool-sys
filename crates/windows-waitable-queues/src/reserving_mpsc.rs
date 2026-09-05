// Copyright (c) Mike Grier.

//! The multi-producer, single-consumer bounded array queue **that can reserve**.
//!
//! Everything [`slotwise_mpsc`](crate::slotwise_mpsc) is, plus [`Producer::reserve`]: a slot
//! claimed in advance, so that a later delivery cannot be refused for want of
//! room. *Reserved is guaranteed, unreserved is best-effort.*
//!
//! # Known defect: this shape can lose an item after 2^32 pushes
//!
//! **On every target, not only 32-bit ones** -- the claim position is a 32-bit
//! half of the packed word below by construction, so this reaches x86-64 and
//! ARM64 exactly as it reaches i686.
//!
//! A producer that has checked for room, been descheduled, and resumed after
//! other producers drove the position through a full wrap will claim
//! successfully against a numerically identical but generations-later value,
//! and write into a slot whose emptiness was decided long ago. **The failure is
//! silent**: the consumer receives a different item than was sent, and no error,
//! panic, or counter reports it.
//!
//! 2^32 pushes is 37 seconds to about four minutes of *sustained* pushing at
//! this crate's measured rates, roughly two minutes at two producers. The wrap
//! alone is not enough -- a producer must also stall inside a window a few
//! instructions wide -- but a preemption suffices.
//!
//! [`slotwise_mpsc`](crate::slotwise_mpsc) does not have this hazard, its
//! positions being 64 bits on every target; [`spsc`](crate::spsc) never had it.
//! Below the wrap this shape is sound. The full statement, and what to do about
//! it, is in the [crate documentation](crate).
//!
//! # Why this is a separate shape rather than a method on `slotwise_mpsc`
//!
//! Because the two ask different questions to claim a slot, and only this one's
//! question can answer a reservation. They are two claim protocols, not one
//! queue with a switch.
//!
//! Honouring a reservation costs the producer a read of the consumer's position
//! on **every** push, including the pushes that never reserve anything -- which
//! is what `slotwise_mpsc` avoids and why it cannot offer reservation at all.
//!
//! **That cost is not what makes either shape slower.** This one measured
//! *faster* than `slotwise_mpsc` under contention on both architectures tried, by up to
//! 6.4x, because the slot sequence `slotwise_mpsc` reads instead marches through memory
//! while other producers write it. See the crate documentation for the numbers
//! and for how to choose.
//!
//! `slotwise_mpsc`'s producer never reads the consumer's position. It asks a different
//! question -- "is the slot I am about to claim free?" -- and reads that from
//! the slot's own sequence number, which is spread across the slot array, so
//! producers working at different positions touch different cache lines.
//! Avoiding a single shared position is not incidental to that design; it is
//! most of the point of it.
//!
//! A reservation cannot be honoured from that question. "Is this slot free" does
//! not tell you **how many** slots remain, and holding one back for a reserver
//! requires exactly that count -- which requires the consumer's position, on one
//! line every thread in the system touches.
//!
//! So the two ship as peers ([D-16](../DESIGN-NOTES.md#d-16)): `slotwise_mpsc` for a
//! caller who wants the cheapest possible push and can treat a refusal as
//! backpressure, this shape for a caller with a message it must not lose. That
//! is the narrow-trait argument from [D-2](../DESIGN-NOTES.md#d-2) reaching
//! its sharpest case -- `slotwise_mpsc` does not implement
//! [`Reserving`](crate::Reserving) because it genuinely cannot, not because
//! nobody got round to it.
//!
//! # The claim word, which is why reservation is sound here
//!
//! The reservation count and the claim position live in **one** [`AtomicU64`]:
//! the low 32 bits are the position, the high 32 the number of outstanding
//! reservations. Every operation that changes either changes both together, with
//! one compare-and-swap.
//!
//! That is not tidiness, it is the correctness argument, and the obvious
//! alternative is broken in a way worth recording. With the count in its own
//! atomic:
//!
//! 1. A pushing producer reads the count, sees room, and claims the position.
//! 2. A reserving producer increments the count, reads the position, sees room,
//!    and hands out the reservation.
//!
//! Each read before the other's write, and the queue now owes a slot that does
//! not exist. **Sequentially consistent fences do not close this**, unlike the
//! superficially similar hazard in the internal `Doorbell`: the
//! Dekker argument needs store-then-load on both sides, and the pushing producer
//! is load-then-store -- it *reads* the count and then *writes* the position. In
//! a total order over the four operations, both sides missing each other is
//! consistent, so no fence forbids it. Two independent claimants on one resource
//! must synchronise on one location, so the count and the position become one
//! location.
//!
//! With that, redeeming a reservation is a single compare-and-swap that
//! decrements the count and advances the position at once -- so the quantity the
//! invariant is about, `occupied + reserved`, is never momentarily wrong.
//!
//! # What the packing costs, and what it does not
//!
//! Splitting a 64-bit word 32/32 caps this shape at
//! a maximum of 2^31 items, and that split is forced rather than chosen:
//! a position of `b` bits keeps a wrapping difference unambiguous only up to
//! `2^(b-1)`, and the count needs `b` bits because it can reach the capacity, so
//! `b + b = 64` gives `b = 32`. There is no cleverer division of the word.
//!
//! **A 128-bit compare-and-swap is deliberately not used *here***
//! ([D-37](../DESIGN-NOTES.md#d-37)). It would not remove the cost that
//! matters -- the consumer's position still has to be read -- and 2^31 slots is
//! a ring this shape allocates in full at construction.
//!
//! The operative reason is that widening *this* shape's word would change what
//! it offers depending on the target: `i686-pc-windows-msvc` has no lock-free
//! 128-bit exchange, so the same module would be lock-free on one target and
//! silently mutex-backed on another. A wider claim ships instead as its own
//! shape (`reserving_mpsc_wide`, not yet built -- see D-37), to exist only
//! where the exchange is genuinely lock-free. That keeps *this* module's
//! contract the same on every target, which is the property being protected
//! here: a caller who wants 2^62 slots and no wrap hazard will ask for it by
//! name rather than get it by accident of where they compiled.

use core::cell::{Cell, UnsafeCell};
use core::fmt;
use core::marker::PhantomData;
use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::io;
use std::os::windows::io::{BorrowedHandle, OwnedHandle};
use std::sync::Arc;
use std::time::Duration;

use crate::CacheAligned;
use crate::blocking::{self, Parked};
use crate::capacity::{Bounds, MAX_ADMISSIBLE_CAPACITY, WRAPPING_MAX_CAPACITY, validate_capacity};
use crate::disposal::Teardown;
use crate::doorbell::Doorbell;
use crate::error::{CapacityError, Disconnected, PushError, RecvError, RecvTimeoutError};
use crate::metrics::Metrics;
use crate::options::Options;

/// How many of the claim word's bits carry the position.
///
/// The other half carries the outstanding-reservation count. Changing this
/// changes [`BOUNDS`] and is a breaking change to the capacities this shape
/// accepts; see the [module documentation](self) for why an even split is the
/// only sensible one.
const POSITION_BITS: u32 = 32;

/// Isolates the position half of the claim word.
const POSITION_MASK: u64 = (1 << POSITION_BITS) - 1;

/// The position after `position`, wrapping at the width the packing gives it.
///
/// **Centralised because the width is no longer the type's.** A position is
/// carried in a `u64` but is only [`POSITION_BITS`] wide, so it wraps where the
/// packing says rather than where `u64` would. Spelling that as
/// `wrapping_add(1) & POSITION_MASK` at each of the dozen sites that need it
/// would be a dozen chances to omit the mask, and an omitted mask is not a
/// compile error -- it is a position that escapes its half of the claim word
/// and silently corrupts the reservation count beside it.
const fn advance(position: u64) -> u64 {
    position.wrapping_add(1) & POSITION_MASK
}

/// How far `position` leads `head`, in the modular arithmetic the position
/// width defines.
///
/// Masked for [`advance`]'s reason. When the position was a `u32` the type
/// supplied this wrap for free; it no longer does.
const fn distance(position: u64, head: u64) -> u64 {
    position.wrapping_sub(head) & POSITION_MASK
}

/// What this shape accepts as a capacity.
///
/// The minimum is two for the same reason [`slotwise_mpsc`](crate::slotwise_mpsc)'s is: with a
/// single slot, "published at `p`" and "free again on the next lap" would be the
/// same sequence number.
///
/// The maximum is 2^31 rather than the crate-wide bound, because a position is
/// half of the packed claim word rather than a whole [`usize`]. A wrapping
/// 32-bit difference is unambiguous only up to 2^31, and that is exactly the
/// most items this shape can hold.
///
/// **Bounded by the `usize` width as well as by the packing, because on a
/// 32-bit target the packing is the *wider* of the two.** The crate-wide
/// ceiling below which a wrapping position difference stays unambiguous is
/// `usize::MAX / 2`, which on a 32-bit target is `2^31 - 1` -- narrower than
/// the packing. A flat `1 << 31` therefore exceeds it, and the const assertion
/// below rejects it, failing the build for every capacity including the small
/// valid ones. Taking the narrower of the two limits keeps this a power of two
/// on every target, which matters because the value is offered to a caller as a
/// capacity it could actually use.
pub const BOUNDS_MAX: usize = {
    let packed = 1_usize << (POSITION_BITS - 1);
    if packed <= MAX_ADMISSIBLE_CAPACITY {
        packed
    } else {
        MAX_ADMISSIBLE_CAPACITY
    }
};

/// The capacities this shape accepts. See [`BOUNDS_MAX`].
const BOUNDS: Bounds = Bounds {
    min: 2,
    max: BOUNDS_MAX,
};

/// The largest reservation count the word's other half can hold.
const MAX_RESERVED: u64 = u64::MAX >> POSITION_BITS;

// The relationships the packing depends on, checked by the compiler rather than
// by a test. They are facts about constants, so a test could only ever report
// after the fact, on a build somebody chose to run; here, moving the split
// without re-deriving what depends on it does not compile.
//
// **Note what is deliberately NOT asserted.** That `BOUNDS_MAX` equals
// `1 << (POSITION_BITS - 1)` is tautological -- it is the definition -- and an
// earlier version of this block asserted exactly that, which is to say nothing.
// Widening the position to 40 bits sailed past it while silently narrowing the
// reservation field to 24, which is the real breakage. The assertions below are
// the ones that catch it.
const _: () = {
    assert!(
        POSITION_BITS >= 32,
        "the reservation count is read out as a u32, so a field wider than 32 bits would be \
         truncated on the way out"
    );
    assert!(
        MAX_RESERVED <= u32::MAX as u64,
        "the count is read back out through `reserved_of`'s cast to `u32`, so a field the word \
         could hold but the cast could not would make this constant's name a lie -- and the \
         assertion below would then be satisfied by a ceiling that truncates on the way out"
    );
    assert!(
        BOUNDS_MAX as u64 <= MAX_RESERVED,
        "every slot may be reserved at once, so the count's half of the word must be able to hold \
         the whole capacity -- widening the position narrows this and is the way the packing \
         actually breaks"
    );
    assert!(
        BOUNDS.max <= WRAPPING_MAX_CAPACITY,
        "a shape may be narrower than the crate-wide bound but never wider"
    );
    assert!(
        BOUNDS.max.is_power_of_two(),
        "the maximum is offered to a caller as a capacity it could use, so it must itself be one \
         this shape would accept -- and on a 32-bit target the crate-wide ceiling is not a power \
         of two, so clamping to it directly would have produced a suggestion that is rejected"
    );
    assert!(
        BOUNDS.min <= BOUNDS.max,
        "a shape that accepts nothing would reject every capacity with a suggestion it would also \
         reject"
    );
    // The clamp's own shape -- that it is the *widest* such power of two -- is
    // asserted where it is defined, in `capacity::MAX_ADMISSIBLE_CAPACITY`, so
    // every shape that clamps against it inherits the check rather than
    // restating it.
};

/// Reads the position out of a claim word.
const fn position_of(word: u64) -> u64 {
    word & POSITION_MASK
}

/// Reads the outstanding-reservation count out of a claim word.
const fn reserved_of(word: u64) -> u32 {
    (word >> POSITION_BITS) as u32
}

/// Builds a claim word from its two halves.
///
/// **Why the word is one `AtomicU64` and not two `AtomicU32`s**, given that every
/// operation on it is `Relaxed` (see D-38 in DESIGN-NOTES.md): relaxed is a
/// statement about *ordering*, and says nothing about atomicity. The two halves
/// are read and written as a unit, so the load must be indivisible -- a torn read
/// would return a `(reserved, position)` pair that was never a state this queue
/// was in, and the compare-and-swap protocol would be building on a value that
/// never existed. On `i686-pc-windows-msvc`, which D-18 keeps supported, that
/// costs a `cmpxchg8b` or an 8-byte SSE load rather than the two `mov`s a plain
/// `u64` would get. That cost is the point, not an overhead to optimize away.
///
/// The `|` could equally be `^`, or `+`, and a mutation run will report as much.
/// The halves are disjoint by construction -- the shift clears every bit the
/// position occupies -- so all three agree on every input, and no test can tell
/// them apart. `|` is kept because it says "these are separate fields" where the
/// others say "these are numbers"; the equivalence is recorded here so it is not
/// investigated again.
const fn claim_word(reserved: u32, position: u64) -> u64 {
    ((reserved as u64) << POSITION_BITS) | (position & POSITION_MASK)
}

/// Creates a reserving multi-producer, single-consumer bounded array queue.
///
/// One producer handle is returned; further producers are made by cloning it,
/// and the queue is disconnected when the last of them -- and the last
/// outstanding [`Reservation`] -- is gone.
///
/// `capacity` must be a power of two between two and [`BOUNDS_MAX`], and is the
/// exact number of items the queue holds -- not a hint, and not rounded.
///
/// # Errors
///
/// Returns [`CapacityError`] if `capacity` is zero, is not a power of two, is
/// less than two, or exceeds [`BOUNDS_MAX`].
///
/// # Examples
///
/// A slot taken before the work that will fill it, so the delivery cannot fail
/// for want of room:
///
/// ```
/// use windows_waitable_queues::reserving_mpsc;
///
/// let (tx, rx) = reserving_mpsc::bounded::<u32>(2)?;
///
/// // Claimed up front, while failing is still cheap.
/// let slot = tx.reserve().expect("a fresh queue has room");
///
/// // The rest of the queue fills. Best-effort pushes cannot take the
/// // reserved slot, so one of these is refused.
/// tx.push(1).expect("one slot remains unreserved");
/// assert!(tx.push(2).is_err(), "the other belongs to the reservation");
///
/// // And the reservation is still honoured, on a queue that is otherwise full.
/// slot.send(99).expect("the room was already ours");
///
/// assert_eq!(rx.pop(), Some(1));
/// assert_eq!(rx.pop(), Some(99));
/// # Ok::<(), windows_waitable_queues::CapacityError>(())
/// ```
pub fn bounded<T>(capacity: usize) -> Result<(Producer<T>, Consumer<T>), CapacityError> {
    build(capacity, Options::new())
}

/// Creates a queue with something other than the default behaviour.
///
/// Identical to [`bounded`] except for what [`Options`] asks for.
///
/// **This is the shape where disposal matters most.** A reservation exists
/// because its message must not be lost; a message redeemed into a queue that
/// is then torn down undrained would be lost after all, just later and more
/// quietly. Pairing a reservation with a disposal sink is what closes that.
///
/// [`Options::tracking_high_water`] costs this shape almost nothing, unlike
/// [`slotwise_mpsc`](crate::slotwise_mpsc): the producer already reads the consumer's position
/// to decide whether there is room beyond the reservations, so the depth is a
/// subtraction of two numbers it is already holding.
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
            // Anything that is not `position + 1` for the position this slot
            // first serves, so the consumer sees it as unpublished. The
            // position's own value is the natural choice and matches the state
            // the slot returns to on every later lap.
            sequence: AtomicU64::new(index as u64),
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
        claim: CacheAligned(AtomicU64::new(claim_word(0, 0))),
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

/// One cell of the ring: an item, and a sequence number saying whether it has
/// been published.
struct Slot<T> {
    /// `position + 1` once the producer that claimed `position` has finished
    /// writing, and anything else before that.
    ///
    /// **This shape uses the sequence for one direction only.** In
    /// [`slotwise_mpsc`](crate::slotwise_mpsc) it answers both "has this been published?" for the
    /// consumer and "is this slot free?" for the producer. Here the producer
    /// answers the second from the consumer's position instead -- it has to read
    /// that position anyway, to count free slots for the reservations -- so
    /// nothing ever stores a "free again" value and the consumer's `pop` is one
    /// store shorter than `slotwise_mpsc`'s.
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
    /// Padded onto its own cache line, and here the padding earns its place
    /// twice over: unlike `slotwise_mpsc`, *every* producer reads this on *every* push,
    /// so letting the claim word share the line would put the consumer's writes
    /// directly in their path.
    head: CacheAligned<AtomicU64>,
    /// The outstanding-reservation count and the claim position, packed.
    ///
    /// One word because they must be claimed together; see the [module
    /// documentation](self) for why two atomics cannot be made correct with any
    /// amount of fencing.
    claim: CacheAligned<AtomicU64>,
    /// How many producer handles and outstanding reservations are alive.
    ///
    /// **A reservation counts as a producer**, which is not bookkeeping
    /// pedantry: a reservation is a promise of a message still to come, so a
    /// consumer that saw the stream end while one was outstanding would be told
    /// the queue was finished and then handed an item. That would lose exactly
    /// the message the reservation existed to protect.
    producers: AtomicUsize,
    consumer_live: AtomicBool,
    /// Readiness as a waitable `HANDLE`. Costs nothing until somebody asks for
    /// the handle, so a polling consumer never allocates a kernel object.
    doorbell: Doorbell,
}

// SAFETY: a slot is written by exactly one producer -- the one whose
// compare-and-swap claimed that position -- and read by exactly one consumer,
// which reads it only after observing the release store of `position + 1` that
// publishes it. The write of the item therefore happens-before the read, and no
// two threads ever touch the same slot's contents at the same time. `T: Send` is
// required and sufficient because an item is moved between threads and never
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
    /// The capacity as the width the positions are counted in.
    ///
    /// Lossless by construction: [`BOUNDS`] caps the capacity at 2^31.
    fn capacity_u64(&self) -> u64 {
        debug_assert!(self.capacity <= BOUNDS_MAX);
        self.capacity as u64
    }

    /// Whether a *best-effort* claim may take the slot at `position`, given the
    /// reservations currently outstanding.
    ///
    /// Written as a subtraction from the capacity rather than as
    /// `occupied + reserved >= capacity`, because both terms can reach 2^31 and
    /// their sum would overflow the width the positions are counted in. The
    /// invariant guarantees `reserved <= capacity`, so this cannot underflow.
    ///
    /// **The answer is only meaningful for a claim word that is still current.**
    /// `position` comes from a claim word and `head` is read here, so the two
    /// need not describe the same instant: if other producers claim and publish
    /// past a stale `position` and the consumer drains them, `head` overtakes it
    /// and the subtraction wraps to near [`u32::MAX`] -- "full" computed from a
    /// pair of readings that never coexisted. Callers therefore treat a `false`
    /// as provisional and re-read the claim before reporting it (see
    /// [`Producer::push`]).
    fn has_room_beyond_reservations(&self, position: u64, reserved: u32) -> bool {
        let capacity = self.capacity_u64();
        debug_assert!(
            u64::from(reserved) <= capacity,
            "reservations may never exceed the capacity they are claimed from"
        );
        let occupied = distance(position, self.head.0.load(Ordering::Acquire));
        occupied < capacity - u64::from(reserved)
    }

    /// Items currently held, as a snapshot.
    ///
    /// Counts slots a producer has claimed but not yet finished writing, for the
    /// reason `slotwise_mpsc`'s does: counting only published items would need a walk of
    /// the ring, and this number is a metric rather than a control-flow input.
    ///
    /// **Clamped to the capacity**, for the reason given on `slotwise_mpsc`'s
    /// twin: the claim word and `head` are two loads at two instants, so a
    /// consumer draining past the sampled position makes the wrapping
    /// subtraction produce a number near `u32::MAX`. A bounded queue must never
    /// report holding more than it can.
    fn len(&self) -> usize {
        let position = position_of(self.claim.0.load(Ordering::Relaxed));
        let head = self.head.0.load(Ordering::Acquire);
        (distance(position, head) as usize).min(self.capacity)
    }

    /// How many further items a best-effort push could still place, as a
    /// snapshot.
    ///
    /// **Not `capacity - len()`, which is what the [`Bounded`](crate::Bounded)
    /// default computes and is wrong for this shape.** `len` deliberately
    /// excludes outstanding reservations, so on an empty queue of four with one
    /// slot reserved the default answers four while only three items fit -- and
    /// a caller sizing a batch from it would be told there is room the
    /// reservation is holding.
    ///
    /// The claim word is read **once**: the position and the reservation count
    /// share it precisely so the two cannot be sampled at different instants,
    /// and reading it twice would reintroduce the skew this shape packs them
    /// together to avoid. `head` is still a second load, so the result is
    /// clamped for the reason `len` is.
    fn remaining(&self) -> usize {
        let word = self.claim.0.load(Ordering::Relaxed);
        let head = self.head.0.load(Ordering::Acquire);
        let capacity = self.capacity_u64();
        let occupied = distance(position_of(word), head).min(capacity);
        let spoken_for = occupied.saturating_add(u64::from(reserved_of(word)));
        capacity.saturating_sub(spoken_for) as usize
    }

    /// Whether the consumer would find an item right now.
    ///
    /// Asks precisely what [`Consumer::pop`] asks -- is the slot at the head
    /// position published? A claimed-but-unpublished slot answers `false`, which
    /// is the right answer: the consumer may safely park on it, because the
    /// producer's publishing store is followed by a signal.
    fn has_ready_item(&self) -> bool {
        // Acquire, matching every other load of `head`. This thread is `head`'s
        // only writer, so coherence alone would make a relaxed load read its
        // own latest value -- but `head` carries a release store (in `pop`), and
        // a relaxed load on an atomic that also carries acquire/release
        // operations is a plain load: unanchored, free to be moved by the
        // optimizer or the processor, with no defined position relative to the
        // ordered operations on the same object. Uniform acquire is what makes
        // the load mean, at this point in the source, what it appears to mean.
        let position = self.head.0.load(Ordering::Acquire);
        let slot = &self.slots[position as usize & self.mask];
        slot.sequence.load(Ordering::Acquire) == advance(position)
    }

    /// Give up one unit of the producer count, signalling if it was the last.
    ///
    /// Shared by [`Producer`] and [`Reservation`] because they are the same
    /// obligation: both represent a message that may still arrive, and the last
    /// of either to leave is the one that ends the stream.
    fn release_producer(&self) {
        // `AcqRel` carries both halves. The release half publishes everything
        // this producer pushed to whichever thread observes the count reaching
        // zero, so a consumer that sees the disconnection can trust that
        // draining to empty really has drained everything. The acquire half
        // makes *this* thread -- when it is the one that drives the count to
        // zero -- see the other producers' pushes, which is what makes the
        // signal below meaningful.
        if self.producers.fetch_sub(1, Ordering::AcqRel) != 1 {
            return;
        }

        // Disconnection is a wakeup like any other, and the only one nobody else
        // can deliver. A consumer blocked on the doorbell would otherwise wait
        // forever for an item that can no longer be sent.
        //
        // Only the *last* departure rings: an earlier one changes nothing a
        // consumer could act on, and waking it to discover that would be a
        // spurious wakeup per departing thread.
        self.doorbell.signal();
    }

    /// Write an item into a claimed position and publish it.
    ///
    /// # Safety
    ///
    /// The caller must have claimed `position` by advancing the claim word, and
    /// must not have published it already. A position is claimed by exactly one
    /// producer, so this is the only writer of the slot.
    unsafe fn publish(&self, position: u64, item: T) {
        // Gated, matching `slotwise_mpsc`: untracked, this costs one predictable
        // branch on a field written once at construction, and the shared `head`
        // line is not touched at all.
        //
        // **Before the publication below, and that placement is load-bearing.**
        // The subtraction is only non-negative while the consumer cannot have
        // passed `position`, and what holds it back is precisely that `position`
        // is not published yet. Taken afterwards, the consumer is free to drain
        // past it, the subtraction wraps, and `fetch_max` keeps a vast number
        // forever -- the defect `slotwise_mpsc`'s twin comment records measuring.
        //
        // **Clamped, which its twin does not need to be.** There, the producer's
        // acquire load of the slot's sequence synchronizes-with the consumer
        // freeing that slot, so the `head` read here cannot be older than
        // `position - capacity + 1` and the depth is bounded by construction.
        // This shape has a second entry point with no such edge:
        // [`Reservation::send`] redeems without a room check, so the only `head`
        // its thread is ordered against is the one *`reserve`* read -- which may
        // be arbitrarily old by the time the reservation is redeemed. A stale
        // read can only over-report, never under-report, so clamping to the
        // capacity keeps the value an upper bound on a depth the queue really
        // reached rather than an unbounded one. See [`Observable::high_water`]
        // for what that bound is contracted to mean.
        //
        // [`Observable::high_water`]: crate::Observable::high_water
        // **This load is unconditional because the slot write below needs it,
        // not because the metric does.** It is the acquire half of the pair
        // [`Consumer::pop`] describes: freeing a slot is `head.store(Release)`,
        // and a producer may only write that slot after acquiring a `head` that
        // has passed it. [`Producer::push`] gets that edge from the room check
        // in [`Self::has_room_beyond_reservations`]; [`Reservation::send`]
        // deliberately has no room check, so without this load its non-atomic
        // write would race the consumer's non-atomic read of the previous
        // occupant -- a data race, and undefined behaviour, however reliably a
        // given target's codegen happens to order it today.
        //
        // Placing it here rather than in `send` covers every path with one
        // load.
        //
        // **The load must be fresh *enough*, and a single acquire load does not
        // guarantee that.** An earlier version of this comment argued that
        // because the claim invariant makes `head >= position - capacity + 1`
        // true at the exchange, and `head` never moves backwards, a later load
        // "can only be fresher". That conflates what `head` *is* in modification
        // order with what a load is *guaranteed to observe*: an acquire load may
        // legally return any earlier value in the modification order, and
        // synchronizes only with the release store whose value it actually
        // reads.
        //
        // Nothing else forces freshness here. The claim exchange is `Relaxed`,
        // so it carries no edge; and while `reserve` does read `head`, a
        // `Reservation` is `Send`, so the thread that redeems one **need never
        // have read `head` at all** -- leaving no coherence constraint to
        // inherit. A reservation held across a full lap and redeemed elsewhere
        // is exactly the case. Raised in PR #56 review.
        //
        // So the load is repeated until it observes a `head` that has actually
        // passed this position's previous occupant. That is the store which
        // frees the slot, so observing it (or any later one, by the same
        // consumer and therefore sequenced after its read) is precisely the
        // edge the write below needs. The loop terminates because the claim
        // invariant makes the condition already true in modification order --
        // this waits to *see* it, not for it to *become* true.
        let mut head = self.head.0.load(Ordering::Acquire);
        while distance(position, head) >= self.capacity_u64() {
            std::hint::spin_loop();
            head = self.head.0.load(Ordering::Acquire);
        }
        if self.metrics.tracks_high_water() {
            let depth = (distance(position, head) + 1) as usize;
            // **The clamp is unreachable from here, and is kept deliberately.**
            // The wait above exits only once `position - head < capacity`, so
            // `depth <= capacity` already holds and `min` never binds. It was
            // load-bearing when this was a single unvalidated load: a stale
            // `head` then made the depth an unbounded over-report, and
            // `the_high_water_mark_never_exceeds_the_capacity` drove exactly
            // that. Waiting for a fresh `head` removes the over-report at its
            // source, so that test was replaced by
            // `publish_waits_for_a_head_that_has_freed_the_slot`, which asserts
            // the fix instead of the mitigation.
            //
            // Kept because it costs one register-to-register `min` on a path
            // already doing an atomic load, and because it bounds the metric by
            // the shape's own contract rather than by an argument a future
            // change to the wait might invalidate silently. A mutation run will
            // report it as a survivor; that is expected, and it is unreachable
            // code rather than a missing test.
            self.metrics.record_depth(depth.min(self.capacity));
        }

        let slot = &self.slots[position as usize & self.mask];
        // SAFETY: the caller's claim makes this thread the only writer, and the
        // acquire load of `head` above -- repeated until it observed a value
        // past this position's previous occupant -- synchronizes-with the
        // `head.store` by which the consumer freed this slot a lap ago, so its
        // read of the previous occupant happens-before this write.
        //
        // The claim alone is not enough. It establishes that the slot is
        // *logically* free -- `occupied + reserved <= capacity` with
        // `reserved >= 1` -- but a non-atomic write racing a non-atomic read
        // needs a happens-before edge, not merely a logical guarantee that the
        // read is over. The load above is that edge.
        unsafe {
            (*slot.value.get()).write(item);
        }

        // Release, and this is the publication: it must come after the write,
        // and this is what forbids the compiler and the processor from moving it
        // earlier. Until it lands, the consumer sees the slot as
        // claimed-but-empty and skips it.
        slot.sequence.store(advance(position), Ordering::Release);

        // After the publication, never before: the doorbell says "there is
        // something to take", and that must not become true before the item is
        // actually takeable. A consumer woken early would find nothing, clear
        // the doorbell, and go back to sleep on an item that is about to exist.
        //
        // A producer may signal while an *earlier* position is still
        // unpublished, so the consumer wakes and finds nothing. That is a
        // spurious wakeup, which the protocol tolerates by construction: the
        // producer holding the earlier slot signals in its turn.
        self.doorbell.signal();
    }
}

impl<T> Drop for Shared<T> {
    fn drop(&mut self) {
        // Every handle is gone, so no synchronization is needed and the
        // positions can be read directly. A slot between the two positions still
        // holds an item nobody took, and dropping the queue must drop those
        // rather than leak them.
        //
        // The sequence is consulted per slot rather than assuming every position
        // in the range holds an item. A producer cannot be mid-push here -- it
        // would have to hold a handle, and there are none -- so in practice
        // every one does; the check states the invariant the read depends on
        // instead of leaving it to that argument.
        let mask = self.mask;
        let head = *self.head.0.get_mut();
        let tail = position_of(*self.claim.0.get_mut());
        let mut position = head;
        while position != tail {
            let published = advance(position);
            let slot = &mut self.slots[position as usize & mask];
            if *slot.sequence.get_mut() == published {
                // SAFETY: the slot's sequence says the producer finished writing
                // it and the consumer never took it, so it holds an initialized
                // item. It is read exactly once, because `position` advances
                // every iteration and the slot is never read again.
                let item = unsafe { slot.value.get_mut().assume_init_read() };
                self.teardown.dispose(item);
            }
            position = advance(position);
        }
    }
}

/// A writing half of a [`reserving_mpsc`](self) queue.
///
/// [`Clone`], so producers multiply by cloning rather than by sharing: each
/// thread owns its own handle. Not [`Sync`], so a handle is used by one thread
/// at a time.
pub struct Producer<T> {
    shared: Arc<Shared<T>>,
    /// Removes [`Sync`] without removing [`Send`]. A [`Cell`] is exactly that
    /// shape, and no value of it is ever created.
    not_sync: PhantomData<Cell<()>>,
}

impl<T> Producer<T> {
    /// Appends an item, best-effort.
    ///
    /// **Cannot take a reserved slot.** A queue with one free slot and one
    /// outstanding reservation refuses this, which is the reservation doing its
    /// job rather than a malfunction.
    ///
    /// # Errors
    ///
    /// [`PushError::Full`] when no unreserved room remains, which is the
    /// backpressure signal, and [`PushError::Disconnected`] when the consumer is
    /// gone. Either way the item comes back.
    pub fn push(&self, item: T) -> Result<(), PushError<T>> {
        // Relaxed: this load only proposes a claim. The compare-and-swap below
        // is what makes it, and fails if the proposal was stale, so a stale read
        // costs a retry rather than correctness.
        let mut word = self.shared.claim.0.load(Ordering::Relaxed);
        let position = loop {
            let position = position_of(word);
            let reserved = reserved_of(word);
            #[cfg(test)]
            crate::race_hooks::CLAIM.run();

            if !self.shared.has_room_beyond_reservations(position, reserved) {
                // Provisional, not authoritative. `position` came from `word`
                // and `head` was read inside the check, so a `word` that has
                // since moved makes the two readings describe different
                // instants -- and once `head` passes a stale `position` the
                // subtraction wraps, so an *empty* queue reports full. Re-read
                // the claim: if it moved, this answer was computed from a
                // snapshot that never existed, so retry rather than refuse.
                let current = self.shared.claim.0.load(Ordering::Relaxed);
                if current != word {
                    word = current;
                    continue;
                }
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
            if !self.shared.consumer_live.load(Ordering::Acquire) {
                return Err(PushError::Disconnected(item));
            }

            // Relaxed on both sides is sufficient: this exchange orders nothing
            // but the claim itself. The item's visibility comes from the release
            // store that publishes the slot, and the freedom to write the slot
            // comes from the acquire load of `head` inside the room check.
            //
            // The reservation count is carried through unchanged, which is what
            // makes a racing `reserve` fail its own exchange and re-read rather
            // than have its increment silently overwritten.
            match self.shared.claim.0.compare_exchange_weak(
                word,
                claim_word(reserved, advance(position)),
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break position,
                Err(actual) => word = actual,
            }
        };

        // SAFETY: this thread's compare-and-swap claimed `position`, which no
        // other producer can also have claimed, and it has not been published.
        unsafe {
            self.shared.publish(position, item);
        }
        Ok(())
    }

    /// Claims one slot for a message that must not be lost.
    ///
    /// See [`Reserving::reserve`](crate::Reserving::reserve) for what a
    /// reservation is for. The short form: failing here is cheap, because no
    /// work has been started yet, whereas failing at delivery means blocking or
    /// losing the message.
    ///
    /// The queue stays connected while a reservation is outstanding, so a
    /// consumer will not be told the stream ended and then handed the item.
    #[must_use = "a reservation withholds capacity from every other producer until it is used or dropped"]
    pub fn reserve(&self) -> Option<Reservation<T>> {
        let mut word = self.shared.claim.0.load(Ordering::Relaxed);
        loop {
            let position = position_of(word);
            let reserved = reserved_of(word);
            #[cfg(test)]
            crate::race_hooks::CLAIM.run();

            if !self.shared.has_room_beyond_reservations(position, reserved) {
                // Provisional for the reason `push`'s matching check is: a
                // stale `word` and a freshly-read `head` need not describe the
                // same instant, and once `head` passes a stale `position` the
                // subtraction wraps and an empty queue refuses a reservation.
                let current = self.shared.claim.0.load(Ordering::Relaxed);
                if current != word {
                    word = current;
                    continue;
                }
                return None;
            }

            // The position is carried through unchanged: a reservation claims
            // capacity, not an order. Where the item lands is decided when the
            // reservation is redeemed, so a slot held for a long time does not
            // stall everything queued behind it.
            match self.shared.claim.0.compare_exchange_weak(
                word,
                claim_word(reserved + 1, position),
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => {
                    // Relaxed: this thread already holds a live producer handle,
                    // so the count cannot reach zero during this call and no
                    // other thread's decision depends on when the increment
                    // becomes visible. The pairing that matters is in
                    // `release_producer`.
                    self.shared.producers.fetch_add(1, Ordering::Relaxed);
                    return Some(Reservation {
                        shared: Arc::clone(&self.shared),
                        not_sync: PhantomData,
                    });
                }
                Err(actual) => word = actual,
            }
        }
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

    /// Slots currently claimed by a reservation and not yet redeemed, as a
    /// snapshot.
    #[must_use]
    pub fn outstanding_reservations(&self) -> usize {
        reserved_of(self.shared.claim.0.load(Ordering::Relaxed)) as usize
    }

    /// Whether the next best-effort push would be refused, as a snapshot.
    ///
    /// True when the queue is full *or* every remaining slot is reserved, since
    /// those are indistinguishable to a best-effort caller. Advisory only:
    /// another producer may take the last slot between this call and the push.
    #[must_use]
    pub fn is_full(&self) -> bool {
        self.remaining() == 0
    }

    /// How many further items a best-effort push could still place, as a
    /// snapshot.
    ///
    /// **Reservations are subtracted**, unlike `capacity() - len()`: a reserved
    /// slot is spoken for, so counting it as room would promise a push that
    /// [`push`](Self::push) is guaranteed to refuse. Advisory only, like every
    /// other gauge here.
    #[must_use]
    pub fn remaining(&self) -> usize {
        self.shared.remaining()
    }

    /// Whether the consumer has been dropped.
    #[must_use]
    pub fn is_disconnected(&self) -> bool {
        !self.shared.consumer_live.load(Ordering::Acquire)
    }
}

impl<T> Clone for Producer<T> {
    fn clone(&self) -> Self {
        // Relaxed, for the reason given in `reserve`.
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
        f.debug_struct("reserving_mpsc::Producer")
            .field("capacity", &self.capacity())
            .field("len", &self.len())
            .field("reserved", &self.outstanding_reservations())
            .field("producers", &self.shared.producers.load(Ordering::Relaxed))
            .field("disconnected", &self.is_disconnected())
            .finish()
    }
}

impl<T> Drop for Producer<T> {
    fn drop(&mut self) {
        self.shared.release_producer();
    }
}

/// A slot claimed in advance, which [`Reservation::send`] redeems.
///
/// Owned rather than borrowed from the [`Producer`], and [`Send`], because that
/// is the shape the use case has: an operation reserves its completion slot when
/// it is submitted and redeems it from whichever thread the completion arrives
/// on. ([`spsc`](crate::spsc)'s reservation borrows instead, because there the
/// producer handle *is* the single-producer guarantee and letting a reservation
/// outlive it would create a second one.)
///
/// Dropping it returns the slot to the queue.
#[must_use = "a reservation withholds capacity from every other producer until it is used or dropped"]
pub struct Reservation<T> {
    shared: Arc<Shared<T>>,
    /// See [`Producer::not_sync`]. A reservation may be *moved* between threads
    /// but is used by one at a time, exactly like the handle that made it.
    not_sync: PhantomData<Cell<()>>,
}

impl<T> Reservation<T> {
    /// Delivers into the reserved slot.
    ///
    /// **This cannot fail for want of room**, which is the entire purpose: the
    /// slot was withheld from every other producer from the moment the
    /// reservation was taken. See [`Disconnected`] for why that is the only
    /// error and why the type says so.
    ///
    /// # Errors
    ///
    /// [`Disconnected`] if the consumer is gone, carrying the item back so it
    /// can be accounted for rather than silently dropped.
    pub fn send(self, item: T) -> Result<(), Disconnected<T>> {
        if !self.shared.consumer_live.load(Ordering::Acquire) {
            // Dropping `self` on the way out releases the slot and the producer
            // count, which is what should happen: this message is never coming.
            return Err(Disconnected(item));
        }

        // Redeem and claim in ONE exchange: the count falls by one as the
        // position rises by one, so `occupied + reserved` -- the quantity the
        // whole invariant is about -- is never momentarily wrong, and no
        // concurrent producer can observe a state in which this slot looks
        // available.
        //
        // There is no room check here, and its absence is the guarantee. The
        // invariant `occupied + reserved <= capacity` with `reserved >= 1` means
        // `occupied < capacity`, so the slot at this position is one the
        // consumer has already finished with.
        let mut word = self.shared.claim.0.load(Ordering::Relaxed);
        let position = loop {
            let position = position_of(word);
            let reserved = reserved_of(word);
            debug_assert!(
                reserved >= 1,
                "this reservation is outstanding, so the count cannot be zero"
            );

            match self.shared.claim.0.compare_exchange_weak(
                word,
                claim_word(reserved - 1, advance(position)),
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break position,
                Err(actual) => word = actual,
            }
        };

        // SAFETY: the exchange above claimed `position` for this thread alone,
        // and the invariant argued for in the comment means the slot is free.
        unsafe {
            self.shared.publish(position, item);
        }

        // The slot has been given up as part of the exchange above, so the
        // `Drop` that would give it up again must not run. The producer count,
        // however, still has to be released -- this reservation's promise is now
        // fulfilled, and if it was the last outstanding one the stream ends
        // here.
        //
        // **`mem::forget` would be wrong here, and was wrong here**: this type
        // owns an `Arc`, and forgetting it leaks that strong reference, so the
        // shared state is never dropped and every item still in the ring leaks
        // with it. `ManuallyDrop` plus a move-out suppresses only *this type's*
        // `Drop` while leaving the `Arc`'s own to run exactly once.
        let this = core::mem::ManuallyDrop::new(self);
        // SAFETY: `this` is a `ManuallyDrop`, so its own destructor never runs
        // and the field is not read again after this move.
        let shared = unsafe { core::ptr::read(&this.shared) };
        shared.release_producer();
        // `shared` falls out of scope here, releasing the reference this
        // reservation held.
        Ok(())
    }

    /// Whether the consumer has been dropped, so redeeming would fail.
    #[must_use]
    pub fn is_disconnected(&self) -> bool {
        !self.shared.consumer_live.load(Ordering::Acquire)
    }
}

impl<T> fmt::Debug for Reservation<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("reserving_mpsc::Reservation")
            .field("disconnected", &self.is_disconnected())
            .finish()
    }
}

impl<T> Drop for Reservation<T> {
    fn drop(&mut self) {
        // Give the slot back. Only the count moves: the position is untouched,
        // because an unredeemed reservation never occupied a position.
        let mut word = self.shared.claim.0.load(Ordering::Relaxed);
        loop {
            let reserved = reserved_of(word);
            debug_assert!(
                reserved >= 1,
                "this reservation is outstanding, so the count cannot be zero"
            );
            match self.shared.claim.0.compare_exchange_weak(
                word,
                claim_word(reserved - 1, position_of(word)),
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(actual) => word = actual,
            }
        }
        self.shared.release_producer();
    }
}

/// The reading half of a [`reserving_mpsc`](self) queue.
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
    /// and not yet published it. Order is claim order, so waiting is the only
    /// correct answer -- and the producer signals when it publishes, so waiting
    /// is not a gamble.
    pub fn pop(&self) -> Option<T> {
        // Acquire, matching every other load of `head`. Sole-writer coherence
        // would suffice to read this thread's own latest value, but `head` also
        // carries the release store below, and a relaxed load mixed onto such an
        // atomic is a plain load the code generator may move. See
        // `has_ready_item` for the full argument.
        let position = self.shared.head.0.load(Ordering::Acquire);
        let slot = &self.shared.slots[position as usize & self.shared.mask];
        // Acquire: pairs with the producer's release store, so an item it
        // published is visible here.
        if slot.sequence.load(Ordering::Acquire) != advance(position) {
            return None;
        }

        // SAFETY: the sequence says the producer that claimed this position
        // finished writing it, and the release/acquire pair above makes that
        // write visible here. This is the only consumer, and the position is
        // given up below, so the item is read exactly once.
        let item = unsafe { (*slot.value.get()).assume_init_read() };

        // Release, and this is what frees the slot: a producer reads `head` with
        // an acquire load to count free slots, so this store must not become
        // visible before the read above completes, or that producer could claim
        // the position and overwrite an item this thread had not finished
        // taking.
        //
        // Note that nothing stores a "free again" sequence here, unlike `slotwise_mpsc`.
        // Advancing `head` *is* the release, because this shape's producers
        // decide freedom from `head` rather than from the sequence.
        self.shared
            .head
            .0
            .store(advance(position), Ordering::Release);
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

    /// Slots currently claimed by a reservation and not yet redeemed, as a
    /// snapshot.
    ///
    /// Offered on the consumer as well as the producer because it is the
    /// difference between "nothing is coming" and "something was promised":
    /// a drained queue with an outstanding reservation is not an idle one.
    #[must_use]
    pub fn outstanding_reservations(&self) -> usize {
        reserved_of(self.shared.claim.0.load(Ordering::Relaxed)) as usize
    }

    /// How many further items a best-effort push could still place, as a
    /// snapshot.
    ///
    /// The same number [`Producer::remaining`] reports, and offered here for
    /// the same reason `outstanding_reservations` is: a consumer deciding
    /// whether to keep draining wants the producers' view of the room left, and
    /// that view subtracts reservations rather than treating them as free.
    #[must_use]
    pub fn remaining(&self) -> usize {
        self.shared.remaining()
    }

    /// Whether every producer and every outstanding reservation is gone.
    ///
    /// **Check this only after [`Self::pop`] has returned `None`.** A producer
    /// may push and then drop, so a queue can be disconnected and still hold
    /// items; testing this first would discard them.
    #[must_use]
    pub fn is_disconnected(&self) -> bool {
        self.shared.producers.load(Ordering::Acquire) == 0
    }

    /// Borrows the queue's readiness as a waitable `HANDLE`.
    ///
    /// The event is created on the first call, so a consumer that only ever
    /// polls with [`Self::pop`] is charged for no kernel object.
    ///
    /// # Waiting on it correctly
    ///
    /// **Do not simply wait and then drain.** Use [`Self::arm`] to decide
    /// whether waiting is safe, or the wait can miss an item and block forever;
    /// [`spsc::Consumer::doorbell`](crate::spsc::Consumer::doorbell) carries the
    /// worked example, and the protocol is identical here.
    ///
    /// # Errors
    ///
    /// Returns the error from `CreateEventW` on the first call.
    pub fn doorbell(&self) -> io::Result<BorrowedHandle<'_>> {
        self.shared.doorbell.handle()
    }

    /// A duplicate of [`Self::doorbell`] that the caller owns.
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
    /// something arrived in the meantime.
    ///
    /// **`true` is not by itself permission to wait indefinitely.** It answers
    /// only whether a later *push* can be missed, and says nothing about the
    /// end of the stream: with every producer gone it still returns `true`,
    /// having just cleared the single ring their drop left behind. See
    /// [`Waitable::arm`](crate::Waitable::arm) for the four-step protocol an
    /// indefinite wait needs, and the example on [`Self::doorbell`] for it
    /// written out.
    ///
    /// Clearing must come before the check, which is the reverse of the order
    /// that reads naturally; see [D-9](../DESIGN-NOTES.md#d-9).
    ///
    /// # Errors
    ///
    /// Returns the error from `CreateEventW` on the first call.
    pub fn arm(&self) -> io::Result<bool> {
        // Before the clear, and so before the check: a producer running while no
        // event exists skips signalling, so the check has to come after the
        // event exists to catch what that skip left behind.
        self.shared.doorbell.handle()?;
        self.shared.doorbell.clear();
        #[cfg(test)]
        crate::race_hooks::ARM.run();
        // Deliberately not `is_empty`: the question is whether `pop` would find
        // something, and a claimed-but-unpublished slot is not something `pop`
        // can find.
        Ok(!self.shared.has_ready_item())
    }

    /// The last take before reporting the end of the stream.
    ///
    /// Called only after [`Self::is_disconnected`] has returned `true`, which
    /// makes the answer final rather than a snapshot. It guards a race that is
    /// real and narrow: a producer may push *and then* drop in the window
    /// between a receive's first `pop` and its disconnection check.
    fn finish(&self) -> Option<T> {
        self.pop()
    }

    /// Takes the oldest item, blocking until one arrives.
    ///
    /// # Errors
    ///
    /// [`RecvError::Disconnected`] once every producer *and every outstanding
    /// reservation* is gone and the queue is drained. [`RecvError::Io`] if the
    /// doorbell cannot be created or waited on.
    pub fn recv(&self) -> Result<T, RecvError> {
        blocking::recv(self)
    }

    /// Takes the oldest item, blocking until one arrives or the deadline passes.
    ///
    /// # Errors
    ///
    /// [`RecvTimeoutError::Timeout`] if the deadline passes with the queue still
    /// empty, which is not a malfunction. Otherwise as [`Self::recv`].
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
        f.debug_struct("reserving_mpsc::Consumer")
            .field("capacity", &self.capacity())
            .field("len", &self.len())
            .field("reserved", &self.outstanding_reservations())
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

impl<T> crate::Claim for Reservation<T> {
    type Item = T;

    fn send(self, item: T) -> Result<(), Disconnected<T>> {
        Self::send(self, item)
    }

    fn is_disconnected(&self) -> bool {
        Self::is_disconnected(self)
    }
}

impl<T> crate::Reserving for Producer<T> {
    type Item = T;
    type Reservation<'a>
        = Reservation<T>
    where
        Self: 'a;

    fn reserve(&self) -> Option<Reservation<T>> {
        Self::reserve(self)
    }

    fn outstanding_reservations(&self) -> usize {
        Self::outstanding_reservations(self)
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

    // Overridden, because the default `capacity - len` counts a reserved slot
    // as room: `len` excludes reservations by design, so an empty queue of four
    // holding one reservation would answer four while only three items fit.
    fn remaining(&self) -> usize {
        self.shared.remaining()
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

    // The consumer's view has to agree with the producer's: both describe the
    // same queue, and a caller generic over `Bounded` should not get a different
    // answer depending on which handle it holds.
    fn remaining(&self) -> usize {
        Self::remaining(self)
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
