// Copyright (c) Mike Grier.

//! The single-producer, single-consumer bounded ring.
//!
//! The cheapest shape in the crate: neither side ever executes a
//! compare-and-swap, because each owns one of the two positions outright and
//! only *reads* the other's. It is the completion direction of a two-layer
//! ring, where one domain thread produces and one drainer consumes.
//!
//! # The signatures this module fixes for every later shape
//!
//! This is the first shape written, so its method signatures become the ones a
//! capability trait must be able to name. Written down before the type, per
//! [D-3](../DESIGN-NOTES.md#d-3), because a second shape that spells the
//! same operation differently cannot later be unified without breaking one of
//! them:
//!
//! ```text
//! trait Producer {
//!     type Item;
//!     fn push(&self, item: Self::Item) -> Result<(), PushError<Self::Item>>;
//!     fn is_disconnected(&self) -> bool;
//! }
//!
//! trait Consumer {
//!     type Item;
//!     fn pop(&self) -> Option<Self::Item>;
//!     fn is_disconnected(&self) -> bool;
//! }
//!
//! trait Bounded {
//!     fn capacity(&self) -> usize;
//!     fn len(&self) -> usize;
//!     fn is_empty(&self) -> bool;
//! }
//! ```
//!
//! **They have since shipped, and they kept those signatures.**
//! [`slotwise_mpsc`](crate::slotwise_mpsc) was written against this sketch and matched it, which
//! is the validation [D-3](../DESIGN-NOTES.md#d-3) demanded before any trait
//! was allowed to exist. The sketch is left here because it is the artefact
//! that made the check possible: what [`crate::traits`] says now is what this
//! comment said before either type existed.
//!
//! # Why the operations take `&self`
//!
//! `&mut self` would also make single-producer sound, and several SPSC crates
//! spell it that way. It is rejected here because it does not generalize: a
//! multi-producer shape must let several threads push through a shared handle,
//! which `&mut self` forbids. Since one spelling has to serve every shape, the
//! one that serves the widest is chosen.
//!
//! Cardinality is then carried by the auto traits instead, which is
//! [D-4](../DESIGN-NOTES.md#d-4):
//!
//! | | [`Clone`] | [`Send`] | [`Sync`] |
//! |---|---|---|---|
//! | [`Producer`] | no | yes, if `T: Send` | **no** |
//! | [`Consumer`] | no | yes, if `T: Send` | **no** |
//!
//! Not [`Sync`] is what makes "single" true: a handle that cannot be shared
//! between threads and cannot be duplicated is held by exactly one thread. The
//! compiler enforces it, so no documented precondition has to be remembered. A
//! multi-producer shape will relax exactly one cell of that table.

use core::cell::{Cell, UnsafeCell};
use core::fmt;
use core::marker::PhantomData;
use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::io;
use std::os::windows::io::{BorrowedHandle, OwnedHandle};
use std::sync::Arc;
use std::time::Duration;

use crate::CacheAligned;
use crate::blocking::{self, Parked};
use crate::capacity::{Bounds, MAX_ADMISSIBLE_CAPACITY, validate_capacity};
use crate::disposal::Teardown;
use crate::doorbell::Doorbell;
use crate::error::{CapacityError, Disconnected, PushError, RecvError, RecvTimeoutError};
use crate::metrics::Metrics;
use crate::options::Options;

/// What this shape accepts as a capacity.
///
/// The minimum is one, and there is nothing to work around: a single slot is
/// either inside `[head, tail)` or outside it, and those are the only two
/// states this shape's positions have to distinguish. [`slotwise_mpsc`](crate::slotwise_mpsc)
/// needs two, because its slots carry a third state, and that difference is why
/// each shape names its own bounds rather than sharing one pair.
///
/// The maximum is the widest any shape may be, because this one's positions are
/// full-width [`usize`] values with nothing packed beside them.
const BOUNDS: Bounds = Bounds {
    min: 1,
    max: MAX_ADMISSIBLE_CAPACITY,
};

/// Creates a single-producer, single-consumer bounded ring.
///
/// `capacity` must be a power of two, and is the exact number of items the
/// queue holds -- not a hint, and not rounded. See [`CapacityError`] for why a
/// rejection is preferred to rounding.
///
/// # Errors
///
/// Returns [`CapacityError`] if `capacity` is zero, is not a power of two, or
/// exceeds `2^(usize::BITS - 2)`.
///
/// # Examples
///
/// ```
/// use windows_waitable_queues::spsc;
///
/// let (tx, rx) = spsc::bounded::<u32>(2)?;
/// tx.push(7).expect("a fresh queue has room");
/// assert_eq!(rx.pop(), Some(7));
/// assert_eq!(rx.pop(), None);
/// # Ok::<(), windows_waitable_queues::CapacityError>(())
/// ```
pub fn bounded<T>(capacity: usize) -> Result<(Producer<T>, Consumer<T>), CapacityError> {
    build(capacity, Options::new())
}

/// Creates a ring with something other than the default behaviour.
///
/// Identical to [`bounded`] except for what [`Options`] asks for. See
/// [`Disposal`](crate::Disposal) for why undrained items need a decision made
/// here rather than at teardown, and
/// [`Options::tracking_high_water`] for the one switch that costs the push path
/// anything.
///
/// # Errors
///
/// As [`bounded`].
///
/// # Examples
///
/// ```
/// use std::sync::mpsc;
/// use windows_waitable_queues::{Disposal, Options, spsc};
///
/// let (undelivered, reaper) = mpsc::channel();
/// let (tx, rx) = spsc::bounded_with::<u32>(
///     4,
///     Options::new()
///         .disposal(Disposal::new(move |item| {
///             let _ = undelivered.send(item);
///         }))
///         .tracking_high_water(),
/// )?;
///
/// tx.push(1).expect("a fresh queue has room");
/// tx.push(2).expect("a fresh queue has room");
/// assert_eq!(rx.high_water(), Some(2));
///
/// drop((tx, rx));
/// assert_eq!(reaper.into_iter().collect::<Vec<_>>(), vec![1, 2]);
/// # Ok::<(), windows_waitable_queues::CapacityError>(())
/// ```
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
    slots.resize_with(capacity, || UnsafeCell::new(MaybeUninit::uninit()));

    let shared = Arc::new(Shared {
        teardown: Teardown::new(options.disposal),
        metrics: Metrics::new(options.track_high_water),
        slots: slots.into_boxed_slice(),
        mask: capacity - 1,
        capacity,
        head: CacheAligned(AtomicUsize::new(0)),
        tail: CacheAligned(AtomicUsize::new(0)),
        producer_live: AtomicBool::new(true),
        consumer_live: AtomicBool::new(true),
        reserved: AtomicUsize::new(0),
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

struct Shared<T> {
    slots: Box<[UnsafeCell<MaybeUninit<T>>]>,
    mask: usize,
    capacity: usize,
    /// What becomes of undrained items at teardown.
    ///
    /// Read only by [`Shared::drop`], which holds `&mut self`, so it needs no
    /// synchronization and costs the hot paths nothing but its space.
    teardown: Teardown<T>,
    /// The counters this queue keeps about itself. See [`crate::metrics`].
    metrics: Metrics,
    /// Where the consumer will next read. Owned by the consumer.
    head: CacheAligned<AtomicUsize>,
    /// Where the producer will next write. Owned by the producer.
    tail: CacheAligned<AtomicUsize>,
    producer_live: AtomicBool,
    consumer_live: AtomicBool,
    /// Slots claimed by a [`Reservation`] and not yet redeemed.
    ///
    /// **Written only by the producer's thread**, which is what makes
    /// reservation nearly free in this shape: `reserve`, `Reservation::send` and
    /// `Reservation::drop` all run on the single producer, so a plain load and
    /// store suffice where [`reserving_mpsc`](crate::reserving_mpsc) needs a
    /// compare-and-swap against a packed word. The line is exclusive to that
    /// core, so the extra read on the push path costs essentially nothing.
    ///
    /// Atomic rather than a [`Cell`] only because the consumer reads it as a
    /// metric, and a torn read of a metric is still undefined behaviour.
    reserved: AtomicUsize,
    /// Readiness as a waitable `HANDLE`. Costs nothing until somebody asks for
    /// the handle, so a polling consumer never allocates a kernel object.
    doorbell: Doorbell,
}

// SAFETY: the two positions partition the slot array between the threads. A
// slot in `[head, tail)` is owned by the consumer and read exactly once; a slot
// outside it is owned by the producer and written exactly once. Each side
// publishes its position with a release store that the other acquires, so the
// write of an item happens-before the read of that item. `T: Send` is required
// and sufficient because an item is moved between the threads and never
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
    /// Items currently held.
    ///
    /// Both loads are `Acquire` so that a caller on either side sees a value
    /// consistent with the items it can actually observe. It is a snapshot the
    /// moment it is returned: the peer may push or pop immediately afterwards,
    /// which is why nothing here invites a check-then-act.
    ///
    /// **Clamped to the capacity**, for the reason the other shapes' gauges are:
    /// `tail` is read before `head`, so a consumer draining past the sampled
    /// value makes the wrapping subtraction produce a number near `usize::MAX`.
    /// A bounded queue must never report holding more than it can.
    fn len(&self) -> usize {
        let tail = self.tail.0.load(Ordering::Acquire);
        let head = self.head.0.load(Ordering::Acquire);
        tail.wrapping_sub(head).min(self.capacity)
    }

    /// How many further items a best-effort push could still place.
    ///
    /// **Not `capacity - len()`, which is what the [`Bounded`](crate::Bounded)
    /// default computes and is wrong for this shape too.** A reservation
    /// withdraws a slot without becoming an item, so after reserving every slot
    /// the default still answers the full capacity while both `push` and
    /// `reserve` refuse.
    fn remaining(&self) -> usize {
        let held = self.len();
        let reserved = self.reserved.load(Ordering::Relaxed);
        self.capacity.saturating_sub(held.saturating_add(reserved))
    }

    /// Write an item into the slot at `tail` and publish it.
    ///
    /// Shared by [`Producer::push`] and [`Reservation::send`] so that the
    /// ordering argument below is made once. The two differ only in how they
    /// established that there is room -- one checked, the other was promised --
    /// and nothing downstream of that decision should be written twice.
    ///
    /// # Safety
    ///
    /// The caller must have established that the slot at `tail` is free: either
    /// by the room check in `push`, or by holding a reservation.
    unsafe fn publish(&self, tail: usize, item: T) {
        // Free on this shape: the producer owns `tail` and already loaded
        // `head` to decide there was room, so the depth is a subtraction of two
        // values it is holding. The counter's line is producer-owned too, since
        // nothing else writes it.
        self.metrics
            .record_depth(tail.wrapping_sub(self.head.0.load(Ordering::Relaxed)) + 1);

        // SAFETY: the caller's precondition says this slot holds no initialized
        // item, so writing a `MaybeUninit` over it drops nothing.
        unsafe {
            (*self.slots[tail & self.mask].get()).write(item);
        }

        // Release: publishes the slot write to the consumer's acquire load. The
        // store must come after the write, and this is what forbids the
        // compiler and the processor from moving it earlier.
        self.tail.0.store(tail.wrapping_add(1), Ordering::Release);

        // After the release store, never before: the doorbell says "there is
        // something to take", and that must not become true before the item is
        // actually takeable. A consumer woken early would find the queue empty,
        // clear the doorbell, and go back to sleep on an item that is about to
        // exist -- a lost wakeup manufactured by signalling too eagerly.
        //
        // Cheap when it is redundant: `signal` returns without a syscall if the
        // doorbell is already lit, so a producer running ahead of its consumer
        // pays one atomic per push rather than one `SetEvent`.
        self.doorbell.signal();
    }
}

impl<T> Drop for Shared<T> {
    fn drop(&mut self) {
        // Both handles are gone, so no synchronization is needed and the
        // positions can be read directly. Every slot in `[head, tail)` still
        // holds an initialized item that nobody took, and tearing the queue
        // down must account for them rather than leak them.
        //
        // Each is *moved out* and handed to the teardown policy rather than
        // destroyed where it lies. For the default policy the two are the same
        // thing; for a queue whose items own handles they are not, and this is
        // the only place that sees every survivor. See `crate::disposal`.
        let head = *self.head.0.get_mut();
        let tail = *self.tail.0.get_mut();
        let mask = self.mask;
        let mut pos = head;
        while pos != tail {
            // SAFETY: `pos` is in `[head, tail)`, so this slot was written by
            // the producer and never read by the consumer. It is read exactly
            // once, because `pos` advances every iteration, and the slot is
            // never read again afterwards.
            let item = unsafe { (*self.slots[pos & mask].get()).assume_init_read() };
            self.teardown.dispose(item);
            pos = pos.wrapping_add(1);
        }
    }
}

/// The writing half of an [`spsc`](self) ring.
///
/// Neither [`Clone`] nor [`Sync`], which is what makes "single producer" a fact
/// the compiler checks rather than a rule to remember.
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
    /// outstanding [`Reservation`] refuses this, which is the reservation doing
    /// its job rather than a malfunction.
    ///
    /// # Errors
    ///
    /// [`PushError::Full`] when no unreserved room remains, which is the
    /// backpressure signal rather than a malfunction, and
    /// [`PushError::Disconnected`] when the consumer is gone. Either way the
    /// item comes back, so nothing is lost by the refusal.
    pub fn push(&self, item: T) -> Result<(), PushError<T>> {
        // Relaxed: this thread is the only writer of `tail`, so it cannot read
        // a stale value of its own.
        let tail = self.shared.tail.0.load(Ordering::Relaxed);
        // Acquire: pairs with the consumer's release store, so a slot it freed
        // is visible as free here.
        let head = self.shared.head.0.load(Ordering::Acquire);
        // Relaxed, and this is the whole cost of reservation on this shape: the
        // only writer of `reserved` is this thread, so the line is exclusive to
        // this core and cannot hold a stale value of its own.
        let reserved = self.shared.reserved.load(Ordering::Relaxed);

        // The sum cannot overflow: each term is at most the capacity, which is
        // itself at most half of `usize::MAX`.
        if tail.wrapping_sub(head) + reserved >= self.shared.capacity {
            // Report disconnection in preference to fullness: a full queue
            // whose consumer is gone will never drain, and telling the caller
            // to retry would be telling it to spin forever.
            if !self.shared.consumer_live.load(Ordering::Acquire) {
                // Not counted as a refusal: this is the end of the stream, not
                // backpressure, and folding the two together would make a
                // shutting-down queue look like an overloaded one.
                return Err(PushError::Disconnected(item));
            }
            self.shared.metrics.record_refusal();
            return Err(PushError::Full(item));
        }
        if !self.shared.consumer_live.load(Ordering::Acquire) {
            return Err(PushError::Disconnected(item));
        }

        // SAFETY: `tail` is outside `[head, tail)`, so this slot is owned by
        // the producer and holds no initialized item, and the room check above
        // left it unclaimed by any reservation.
        unsafe {
            self.shared.publish(tail, item);
        }
        Ok(())
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

    /// Whether the next best-effort push would be refused, as a snapshot.
    ///
    /// True when the queue is full *or* every remaining slot is reserved, since
    /// those are indistinguishable to a best-effort caller.
    ///
    /// Advisory only. Nothing is gained by testing it before [`Self::push`],
    /// which reports the same condition without the window in between; it is
    /// offered for metrics rather than for control flow.
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

    /// Slots currently claimed by a [`Reservation`] and not yet redeemed.
    #[must_use]
    pub fn outstanding_reservations(&self) -> usize {
        self.shared.reserved.load(Ordering::Relaxed)
    }

    /// Claims one slot for a message that must not be lost.
    ///
    /// See [`Reserving::reserve`](crate::Reserving::reserve) for what a
    /// reservation is for. The short form: failing here is cheap, because no
    /// work has been started yet, whereas failing at delivery means blocking or
    /// losing the message.
    ///
    /// **The reservation borrows this producer**, which is not an arbitrary
    /// choice of ownership. This shape is sound because exactly one thread ever
    /// writes the ring, and the producer handle is what makes that true -- it is
    /// neither [`Clone`] nor [`Sync`]. An owned reservation could be moved to a
    /// second thread while the producer stayed on the first, and then two
    /// threads would be writing. Borrowing pins the producer for as long as any
    /// reservation is outstanding, so the compiler enforces what the shape
    /// requires. [`reserving_mpsc`](crate::reserving_mpsc), which has no such
    /// constraint, hands out an owned reservation instead.
    ///
    /// **The refusal is asserted, not merely described.** A reservation cannot
    /// be moved to another thread, because it borrows a producer that is not
    /// [`Sync`], so `&Producer` is not [`Send`]:
    ///
    /// ```compile_fail
    /// # use windows_waitable_queues::spsc;
    /// let (tx, _rx) = spsc::bounded::<u32>(4).unwrap();
    /// let slot = tx.reserve().expect("room");
    /// // Rejected: moving this would put a second writer on the ring.
    /// std::thread::spawn(move || {
    ///     slot.send(1).ok();
    /// });
    /// ```
    ///
    /// The consumer handle *is* [`Send`], so a blocked receiver can still live
    /// on another thread -- it is only the writing side that is pinned.
    #[must_use = "a reservation withholds capacity from the best-effort path until it is used or dropped"]
    pub fn reserve(&self) -> Option<Reservation<'_, T>> {
        let tail = self.shared.tail.0.load(Ordering::Relaxed);
        let head = self.shared.head.0.load(Ordering::Acquire);
        let reserved = self.shared.reserved.load(Ordering::Relaxed);

        if tail.wrapping_sub(head) + reserved >= self.shared.capacity {
            return None;
        }

        // A plain store, where `reserving_mpsc` needs a compare-and-swap against
        // a packed word: there is only one producer, so `reserve`, `push` and
        // the redemption all run on this thread and cannot interleave with each
        // other. That is the entire difference between the two shapes'
        // reservation machinery, and it is why this one costs nothing.
        self.shared.reserved.store(reserved + 1, Ordering::Relaxed);
        Some(Reservation { producer: self })
    }

    /// Whether the consumer has been dropped.
    #[must_use]
    pub fn is_disconnected(&self) -> bool {
        !self.shared.consumer_live.load(Ordering::Acquire)
    }
}

// Hand-written rather than derived: deriving would demand `T: Debug`, which
// would make a handle to a queue of non-`Debug` items un-printable for no
// reason. The item type is not the handle's business, so the handle reports the
// queue's state instead.
impl<T> fmt::Debug for Producer<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("spsc::Producer")
            .field("capacity", &self.capacity())
            .field("len", &self.len())
            .field("disconnected", &self.is_disconnected())
            .finish()
    }
}

impl<T> Drop for Producer<T> {
    fn drop(&mut self) {
        // Release: everything this producer pushed happens-before a consumer
        // observing the disconnection, so a consumer that sees it can trust
        // that draining to empty really has drained everything.
        self.shared.producer_live.store(false, Ordering::Release);

        // Disconnection is a wakeup like any other, and the only one nobody
        // else can deliver. A consumer blocked on the doorbell would otherwise
        // wait forever for an item that can no longer be sent -- the queue
        // would be correct and the program would still hang.
        self.shared.doorbell.signal();
    }
}

/// A slot claimed in advance, which [`Reservation::send`] redeems.
///
/// Borrows the [`Producer`] that made it, so the producer cannot move to
/// another thread while a claim is outstanding. See [`Producer::reserve`] for
/// why that is a soundness requirement here and not merely a style choice.
///
/// Dropping it returns the slot to the best-effort path.
#[must_use = "a reservation withholds capacity from the best-effort path until it is used or dropped"]
pub struct Reservation<'a, T> {
    producer: &'a Producer<T>,
}

impl<T> Reservation<'_, T> {
    /// Delivers into the reserved slot.
    ///
    /// **This cannot fail for want of room**, which is the entire purpose: the
    /// slot was withheld from the best-effort path from the moment the
    /// reservation was taken. See [`Disconnected`] for why that is the only
    /// error and why the type says so.
    ///
    /// # Errors
    ///
    /// [`Disconnected`] if the consumer is gone, carrying the item back so it
    /// can be accounted for rather than silently dropped.
    pub fn send(self, item: T) -> Result<(), Disconnected<T>> {
        let shared = &self.producer.shared;
        if !shared.consumer_live.load(Ordering::Acquire) {
            // Dropping `self` on the way out releases the slot, which is what
            // should happen: this message is never being delivered.
            return Err(Disconnected(item));
        }

        let tail = shared.tail.0.load(Ordering::Relaxed);
        // SAFETY: the reservation guarantees a free slot -- the room check that
        // granted it withheld one from the best-effort path, and this thread is
        // the only one that could have consumed it since.
        unsafe {
            shared.publish(tail, item);
        }

        // Released only now, after the slot it guaranteed has actually been
        // used. No other thread pushes into this shape, so the moment between
        // the publication and this store is invisible to anything that could
        // act on it; the consumer may see the pair inconsistently, but only as
        // a metric.
        let reserved = shared.reserved.load(Ordering::Relaxed);
        debug_assert!(
            reserved >= 1,
            "this reservation is outstanding, so the count cannot be zero"
        );
        shared.reserved.store(reserved - 1, Ordering::Relaxed);

        // The slot has been given up above, so the `Drop` that would give it up
        // a second time must not run.
        core::mem::forget(self);
        Ok(())
    }

    /// Whether the consumer has been dropped, so redeeming would fail.
    #[must_use]
    pub fn is_disconnected(&self) -> bool {
        self.producer.is_disconnected()
    }
}

impl<T> fmt::Debug for Reservation<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("spsc::Reservation")
            .field("disconnected", &self.is_disconnected())
            .finish()
    }
}

impl<T> Drop for Reservation<'_, T> {
    fn drop(&mut self) {
        let reserved = self.producer.shared.reserved.load(Ordering::Relaxed);
        debug_assert!(
            reserved >= 1,
            "this reservation is outstanding, so the count cannot be zero"
        );
        self.producer
            .shared
            .reserved
            .store(reserved - 1, Ordering::Relaxed);
    }
}

/// The reading half of an [`spsc`](self) ring.
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
    /// `None` does not mean the queue is finished. Pair it with
    /// [`Self::is_disconnected`] to distinguish "empty for now" from "empty for
    /// good"; the order matters, and [`Self::is_disconnected`] documents which
    /// way round.
    pub fn pop(&self) -> Option<T> {
        // Relaxed: this thread is the only writer of `head`.
        let head = self.shared.head.0.load(Ordering::Relaxed);
        // Acquire: pairs with the producer's release store, so an item it
        // published is visible here.
        let tail = self.shared.tail.0.load(Ordering::Acquire);

        if head == tail {
            return None;
        }

        // SAFETY: `head` is in `[head, tail)`, so the producer wrote this slot
        // and released it. It is read exactly once, because `head` advances
        // below before any other read can observe the slot as free.
        let item =
            unsafe { (*self.shared.slots[head & self.shared.mask].get()).assume_init_read() };

        // Release: publishes the slot as free to the producer's acquire load.
        // It must come after the read, or the producer could overwrite an item
        // this thread has not finished taking.
        self.shared
            .head
            .0
            .store(head.wrapping_add(1), Ordering::Release);
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

    /// Whether the producer has been dropped.
    ///
    /// **Check this only after [`Self::pop`] has returned `None`.** A producer
    /// may push and then drop, so a queue can be disconnected and still hold
    /// items; testing this first would discard them. Draining to empty and
    /// then finding the producer gone is the only order that cannot lose an
    /// item, and the release store in the producer's `Drop` is what makes the
    /// preceding pushes visible to a consumer that observes it.
    #[must_use]
    pub fn is_disconnected(&self) -> bool {
        !self.shared.producer_live.load(Ordering::Acquire)
    }

    /// Borrows the queue's readiness as a waitable `HANDLE`.
    ///
    /// This is the point of the crate. The handle is a manual-reset event that
    /// is signalled exactly while the queue has something to take, so it can go
    /// into `WaitForMultipleObjects` beside an I/O completion, a shutdown
    /// event, or a timer -- a wait that no queue with a private parking
    /// primitive can join.
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
    /// # use windows_waitable_queues::spsc;
    /// # use windows_sys::Win32::System::Threading::{WaitForSingleObject, INFINITE};
    /// # use std::os::windows::io::AsRawHandle;
    /// # fn demo(rx: &spsc::Consumer<u32>) -> std::io::Result<()> {
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
    /// `true` means the queue was still empty after the doorbell was cleared,
    /// so any later push is guaranteed to signal. `false` means something
    /// arrived in the meantime: take it instead of waiting.
    ///
    /// **`true` is not by itself permission to wait indefinitely.** It answers
    /// only whether a later *push* can be missed, and says nothing about the
    /// end of the stream: with every producer gone it still returns `true`,
    /// having just cleared the single ring their drop left behind. See
    /// [`Waitable::arm`](crate::Waitable::arm) for the four-step protocol an
    /// indefinite wait needs, and the example on [`Self::doorbell`] for it
    /// written out.
    ///
    /// The order inside this method is the whole correctness argument, and it
    /// is the reverse of the one that reads naturally. Clearing *first* and
    /// checking emptiness *second* is what makes a lost wakeup impossible: an
    /// item that arrives before the clear is found by the check, and an item
    /// that arrives after the clear signals a doorbell that
    /// the internal `clear` has left able to ring.
    /// Checking first would leave a window in which a push both signals and has
    /// its signal erased, and the consumer would sleep on a queue that is not
    /// empty and will never be signalled again.
    ///
    /// The first of those two cases is stronger here than it is for
    /// [`slotwise_mpsc`](crate::slotwise_mpsc): there is one producer and one position, so *any*
    /// push before the clear makes this check find something. That is why this
    /// shape never exhibited the doorbell defect `slotwise_mpsc` exposed, and why the
    /// fix for it belongs to the doorbell rather than to either caller.
    ///
    /// This also creates the doorbell if it does not exist, which must happen
    /// before the emptiness check for the same reason: a producer running while
    /// there is no event skips signalling, so the check has to come after the
    /// event exists to catch what that skip left behind.
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
        Ok(self.is_empty())
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
    /// [`RecvError::Disconnected`] once the producer is gone *and* the queue is
    /// drained -- items pushed before the producer dropped are still delivered.
    /// [`RecvError::Io`] if the doorbell cannot be created or waited on.
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
        f.debug_struct("spsc::Consumer")
            .field("capacity", &self.capacity())
            .field("len", &self.len())
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

impl<T> crate::Claim for Reservation<'_, T> {
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
        = Reservation<'a, T>
    where
        Self: 'a;

    fn reserve(&self) -> Option<Reservation<'_, T>> {
        Self::reserve(self)
    }

    fn outstanding_reservations(&self) -> usize {
        Self::outstanding_reservations(self)
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

    // Overridden, because the default `capacity - len` counts a reserved slot as
    // room: a reservation withdraws capacity without becoming an item, so after
    // reserving every slot the default would answer the full capacity while both
    // `push` and `reserve` refuse.
    fn remaining(&self) -> usize {
        Self::remaining(self)
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
        self.shared.remaining()
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
