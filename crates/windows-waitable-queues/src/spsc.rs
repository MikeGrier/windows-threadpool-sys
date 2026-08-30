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
//! [D-3](../../DESIGN-NOTES.md#d-3), because a second shape that spells the
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
//! The traits themselves are deliberately absent until a second shape exists to
//! validate them.
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
//! [D-4](../../DESIGN-NOTES.md#d-4):
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
use std::sync::Arc;

use crate::CacheAligned;
use crate::error::{CapacityError, PushError};

/// The largest capacity that keeps the producer-minus-consumer difference
/// unambiguous once the positions wrap.
///
/// Positions are monotonic and wrap with the integer, so the number of items
/// held is `tail.wrapping_sub(head)`. That is correct across wraparound only
/// while the true difference cannot exceed half the range.
const MAX_CAPACITY: usize = usize::MAX / 2;

/// Creates a single-producer, single-consumer bounded ring.
///
/// `capacity` must be a power of two, and is the exact number of items the
/// queue holds -- not a hint, and not rounded. See [`CapacityError`] for why a
/// rejection is preferred to rounding.
///
/// # Errors
///
/// Returns [`CapacityError`] if `capacity` is zero, is not a power of two, or
/// exceeds [`usize::MAX`] / 2.
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
    if capacity == 0 {
        return Err(CapacityError::zero());
    }
    if !capacity.is_power_of_two() {
        return Err(CapacityError::not_power_of_two(capacity));
    }
    if capacity > MAX_CAPACITY {
        return Err(CapacityError::too_large(capacity));
    }

    let mut slots = Vec::with_capacity(capacity);
    slots.resize_with(capacity, || UnsafeCell::new(MaybeUninit::uninit()));

    let shared = Arc::new(Shared {
        slots: slots.into_boxed_slice(),
        mask: capacity - 1,
        capacity,
        head: CacheAligned(AtomicUsize::new(0)),
        tail: CacheAligned(AtomicUsize::new(0)),
        producer_live: AtomicBool::new(true),
        consumer_live: AtomicBool::new(true),
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
    /// Where the consumer will next read. Owned by the consumer.
    head: CacheAligned<AtomicUsize>,
    /// Where the producer will next write. Owned by the producer.
    tail: CacheAligned<AtomicUsize>,
    producer_live: AtomicBool,
    consumer_live: AtomicBool,
}

// SAFETY: the two positions partition the slot array between the threads. A
// slot in `[head, tail)` is owned by the consumer and read exactly once; a slot
// outside it is owned by the producer and written exactly once. Each side
// publishes its position with a release store that the other acquires, so the
// write of an item happens-before the read of that item. `T: Send` is required
// and sufficient because an item is moved between the threads and never
// referenced from both.
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
    fn len(&self) -> usize {
        let tail = self.tail.0.load(Ordering::Acquire);
        let head = self.head.0.load(Ordering::Acquire);
        tail.wrapping_sub(head)
    }
}

impl<T> Drop for Shared<T> {
    fn drop(&mut self) {
        // Both handles are gone, so no synchronization is needed and the
        // positions can be read directly. Every slot in `[head, tail)` still
        // holds an initialized item that nobody took, and dropping the queue
        // must drop them rather than leak them.
        let head = *self.head.0.get_mut();
        let tail = *self.tail.0.get_mut();
        let mut pos = head;
        while pos != tail {
            // SAFETY: `pos` is in `[head, tail)`, so this slot was written by
            // the producer and never read by the consumer. It is dropped
            // exactly once, because `pos` advances every iteration.
            unsafe {
                (*self.slots[pos & self.mask].get()).assume_init_drop();
            }
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
    /// Appends an item.
    ///
    /// # Errors
    ///
    /// [`PushError::Full`] when the queue is at capacity, which is the
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

        if tail.wrapping_sub(head) == self.shared.capacity {
            // Report disconnection in preference to fullness: a full queue
            // whose consumer is gone will never drain, and telling the caller
            // to retry would be telling it to spin forever.
            if !self.shared.consumer_live.load(Ordering::Acquire) {
                return Err(PushError::Disconnected(item));
            }
            return Err(PushError::Full(item));
        }
        if !self.shared.consumer_live.load(Ordering::Acquire) {
            return Err(PushError::Disconnected(item));
        }

        // SAFETY: `tail` is outside `[head, tail)`, so this slot is owned by
        // the producer and holds no initialized item. Writing a `MaybeUninit`
        // over uninitialized memory drops nothing.
        unsafe {
            (*self.shared.slots[tail & self.shared.mask].get()).write(item);
        }

        // Release: publishes the slot write to the consumer's acquire load. The
        // store must come after the write, and this is what forbids the
        // compiler and the processor from moving it earlier.
        self.shared
            .tail
            .0
            .store(tail.wrapping_add(1), Ordering::Release);
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

    /// Whether the next push would be refused for want of room, as a snapshot.
    ///
    /// Advisory only. Nothing is gained by testing it before [`Self::push`],
    /// which reports the same condition without the window in between; it is
    /// offered for metrics rather than for control flow.
    #[must_use]
    pub fn is_full(&self) -> bool {
        self.len() == self.shared.capacity
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

#[cfg(test)]
mod tests;
