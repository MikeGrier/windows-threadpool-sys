// Copyright (c) Mike Grier.

//! The capability traits, each naming one thing a queue can do.
//!
//! # Narrow, on the `std::io` model
//!
//! There is deliberately no single `WaitableQueue` trait. `std::io` does not
//! have one `Io` trait either; it has [`Read`](std::io::Read),
//! [`Write`](std::io::Write) and [`Seek`](std::io::Seek), and a type implements
//! the subset it genuinely has. The same choice is forced here rather than
//! merely preferred, because a fat trait is *unimplementable* by shapes this
//! crate plans to ship: a queue that is never waited on has no doorbell to
//! return, and an unbounded one has no capacity to report. Recorded as
//! [D-2](../DESIGN-NOTES.md#d-2).
//!
//! What that buys is a consumer generic over exactly what it needs. A drainer
//! that parks on a queue asks for [`Consumer`] and [`Waitable`], and stays
//! usable against a shape that has never heard of reservation or loss
//! reporting.
//!
//! # Why they arrive with the second shape and not the first
//!
//! A trait written against one implementation designs in a vacuum: every
//! signature that type happens to have looks like a requirement, and nothing
//! tests whether the abstraction is the right one. So the trait *shape* was
//! fixed in prose when `spsc` was written -- the signatures were spelled out in
//! its module documentation before the type existed -- and the traits
//! themselves waited for `slotwise_mpsc` to exist to be checked against. That is
//! [D-3](../DESIGN-NOTES.md#d-3), and the check it demands is not rhetorical:
//! `slotwise_mpsc` is a lock-free multi-producer array queue with no structural
//! resemblance to `spsc` beyond its interface, so a signature that fitted only
//! the first shape would have failed here rather than in a consumer's code.
//!
//! # The name a trait shares with a handle
//!
//! [`Producer`] and [`Consumer`] are also the names of the concrete handles in
//! [`spsc`](crate::spsc) and [`slotwise_mpsc`](crate::slotwise_mpsc). That is deliberate: the
//! trait is named for the role, the handle is named for the role, and the
//! handle plays the role. `std` does the same thing with `fmt::Write` and
//! `io::Write`, and the module path disambiguates. Importing the traits
//! anonymously -- `use windows_waitable_queues::{Bounded as _, Consumer as _}`
//! -- avoids the question entirely when only the methods are wanted.

use std::io;
use std::os::windows::io::{BorrowedHandle, OwnedHandle};

use crate::error::PushError;

/// The writing end of a queue.
pub trait Producer {
    /// What this queue carries.
    type Item;

    /// Appends an item.
    ///
    /// Takes `&self` rather than `&mut self`, which is what lets a
    /// multi-producer shape share one handle's operation across threads. A
    /// single-producer shape gets its guarantee from not being [`Sync`]
    /// instead, so nothing is given up by the weaker receiver.
    ///
    /// # Errors
    ///
    /// [`PushError::Full`] when the queue is at capacity, which is the
    /// backpressure signal rather than a malfunction, and
    /// [`PushError::Disconnected`] when every consumer is gone. Either way the
    /// item comes back, so nothing is lost by the refusal.
    fn push(&self, item: Self::Item) -> Result<(), PushError<Self::Item>>;

    /// Whether every consumer is gone, so nothing will ever take an item again.
    fn is_disconnected(&self) -> bool;
}

/// The reading end of a queue.
pub trait Consumer {
    /// What this queue carries.
    type Item;

    /// Takes the oldest item, or `None` if there is none right now.
    ///
    /// `None` does not mean the queue is finished. Pair it with
    /// [`Consumer::is_disconnected`], and in that order: a producer may push
    /// and then drop, so a queue can be disconnected and still hold items.
    fn pop(&self) -> Option<Self::Item>;

    /// Whether every producer is gone.
    ///
    /// **Ask only after [`Consumer::pop`] has returned `None`.** Draining to
    /// empty and then finding the producers gone is the only order that cannot
    /// lose an item.
    fn is_disconnected(&self) -> bool;

    /// Takes items until the queue is momentarily empty.
    ///
    /// Ends when a [`Consumer::pop`] returns `None`, which is a statement about
    /// this instant and not about the stream: a producer may push again
    /// immediately afterwards. It is the "take everything available" step of
    /// the arming protocol, not a way to consume a queue to its end.
    fn drain(&self) -> Drain<'_, Self>
    where
        Self: Sized,
    {
        Drain { consumer: self }
    }
}

/// Takes items from a [`Consumer`] until it is momentarily empty.
///
/// Created by [`Consumer::drain`].
#[derive(Debug)]
pub struct Drain<'a, C> {
    consumer: &'a C,
}

impl<C: Consumer> Iterator for Drain<'_, C> {
    type Item = C::Item;

    fn next(&mut self) -> Option<Self::Item> {
        self.consumer.pop()
    }
}

/// A queue that holds a fixed number of items and says how many.
///
/// Implemented by both ends, because both have a use for it: a producer reads
/// it to report backpressure, and a consumer to report depth.
pub trait Bounded {
    /// The exact number of items this queue holds when full.
    ///
    /// Not a hint and not rounded -- it is the number the caller asked for.
    fn capacity(&self) -> usize;

    /// Items currently held, as a snapshot.
    ///
    /// A snapshot the moment it is returned: the other end may push or pop
    /// immediately afterwards, which is why nothing here invites a
    /// check-then-act. Use it for metrics, not for control flow.
    fn len(&self) -> usize;

    /// Whether the queue holds nothing, as a snapshot.
    fn is_empty(&self) -> bool;

    /// How many more items would fit, as a snapshot.
    ///
    /// Saturating rather than wrapping, because a shape may count a slot that a
    /// producer has claimed but not yet finished writing, and a momentary
    /// overshoot should read as "no room" rather than as a very large number.
    fn remaining(&self) -> usize {
        self.capacity().saturating_sub(self.len())
    }
}

/// A producer that can claim a slot in advance, so that a later delivery cannot
/// be refused for want of room.
///
/// # What a reservation is for
///
/// A bounded queue refuses when it is full, and that refusal is the
/// backpressure it exists to provide. But not everything travelling a queue can
/// survive being refused the same way. A telemetry sample lost to a full queue
/// is a gap in a chart; an I/O completion lost to a full queue is a caller
/// waiting forever for something that already happened.
///
/// Rather than sort that out per message at the point of delivery -- where the
/// queue is already full and the decision is already too late -- reliability
/// becomes a property of **capacity claimed in advance**. The slot is taken
/// before the work that will fill it is allowed to start, so by the time there
/// is something to deliver, the room is already the holder's. One line covers
/// it: *reserved is guaranteed, unreserved is best-effort.*
///
/// The same discipline, reached independently, is what
/// `windows-file-watcher`'s notification queue runs on.
///
/// # Why this is a trait a shape may lack
///
/// [`slotwise_mpsc`](crate::slotwise_mpsc) deliberately does **not** implement this, and that is
/// the clearest illustration of why the capability traits are narrow
/// ([D-2](../DESIGN-NOTES.md#d-2)). Honouring a reservation means knowing how
/// many slots remain, which costs a producer a read of the consumer's position
/// on every push -- a single line every thread touches. `slotwise_mpsc`'s push avoids
/// that read by design, so it cannot answer the question, and
/// [`reserving_mpsc`](crate::reserving_mpsc) exists beside it for callers who
/// would rather pay than lose a message.
///
/// A fat trait would have forced that cost on both, or excluded the reservation
/// from the contract entirely. Narrow traits let the two ship as peers.
pub trait Reserving {
    /// What this queue carries.
    type Item;

    /// The claim, which is redeemed or released but never ignored.
    ///
    /// **Generic over a lifetime because the two shapes genuinely differ**, and
    /// that difference is the trait being validated by two implementations
    /// rather than shaped around one ([D-3](../DESIGN-NOTES.md#d-3)).
    /// [`reserving_mpsc`](crate::reserving_mpsc) hands out an owned, [`Send`]
    /// reservation, because its use case is to claim a slot when an operation is
    /// submitted and redeem it from whichever thread the completion arrives on.
    /// [`spsc`](crate::spsc) hands out one that borrows the producer, because
    /// there the producer handle *is* the single-producer guarantee: an owned
    /// reservation could outlive it on another thread, and then two threads
    /// would be writing the ring.
    type Reservation<'a>
    where
        Self: 'a;

    /// Claims one slot, or reports that none is available.
    ///
    /// **This is the fallible half, and deliberately so.** Failing here is
    /// cheap: no work has been started and nothing needs delivering, so a
    /// caller can wait, shed load, or refuse the request upstream. That is the
    /// whole trade -- the failure is moved from the moment of delivery, when
    /// the only remaining options are to block or to lose the message, to the
    /// moment of admission, when there are still good ones.
    ///
    /// A claim held is capacity withdrawn from every other producer, so hold it
    /// for as long as correctness needs and no longer. Dropping it returns the
    /// slot.
    #[must_use = "a reservation withholds capacity from every other producer until it is used or dropped"]
    fn reserve(&self) -> Option<Self::Reservation<'_>>;

    /// How many slots are currently claimed and not yet redeemed.
    ///
    /// A snapshot, and offered for metrics rather than for control flow:
    /// [`Reserving::reserve`] answers "can I have one" without the window that
    /// testing this first would open.
    fn outstanding_reservations(&self) -> usize;
}

/// What a queue can report about its own history.
///
/// # Why depth is not here
///
/// [D-2](../DESIGN-NOTES.md#d-2)'s sketch of this trait listed "depth,
/// high-water, doorbells actually rung", and depth has been left off
/// deliberately. [`Bounded::len`] already reports it, computed on demand from
/// positions the queue keeps anyway. Naming it again here would give one number
/// two spellings and two places to drift apart, which is the restatement
/// problem this workspace has paid for before. What belongs here is only what
/// has to be **accumulated** -- facts about the past that the queue's current
/// state cannot reconstruct.
///
/// # Implemented by both ends
///
/// A producer wants to know how often it was refused; a consumer wants to know
/// how deep the backlog got and how often it was actually woken. Both are
/// asking about the same queue, so both handles answer.
pub trait Observable {
    /// How many pushes have been refused for want of room.
    ///
    /// **This is the loss count**, and it is the part of the file watcher's
    /// coalesced loss latch that generalises: a latch can only coalesce losses
    /// that are *idempotent*, which is a property of the payload rather than of
    /// the queue, but counting them needs nothing of the payload at all. See
    /// [D-19](../DESIGN-NOTES.md#d-19).
    ///
    /// Counts refusals for **room** only. A push refused because every consumer
    /// is gone is the end of the stream rather than a loss, and folding the two
    /// together would make a shutting-down queue look like an overloaded one.
    fn refused(&self) -> u64;

    /// How many times the doorbell has actually rung.
    ///
    /// Counts `SetEvent` calls rather than signal attempts, and the difference
    /// between the two *is* the skip optimisation. That is what makes this the
    /// number worth reporting: the skip rule becomes measurable rather than
    /// assumed, and turning the skip off has to move it.
    ///
    /// A queue whose consumer only ever polls never creates its doorbell, so
    /// this stays zero -- which is the laziness being visible rather than a
    /// gap.
    fn doorbell_rings(&self) -> u64;

    /// The deepest the queue has been, if it is being tracked.
    ///
    /// `None` means nobody was counting, which is **not** the same answer as
    /// `Some(0)`. Tracking is off unless
    /// [`Options::tracking_high_water`](crate::Options::tracking_high_water)
    /// asked for it, because a peak has to observe every change and that is the
    /// one metric here which cannot be made free.
    fn high_water(&self) -> Option<usize>;
}

/// A queue whose readiness can be waited on as a Windows `HANDLE`.
///
/// This is the capability the crate is named for, and the reason it exists
/// rather than deferring to an established concurrent-queue crate: a `HANDLE`
/// goes into `WaitForMultipleObjects` beside an I/O completion, a timer, or a
/// shutdown event, and a private parking primitive goes nowhere.
///
/// **Not necessarily queue-specific.** "Hands out a `HANDLE` you can wait on"
/// is equally a property of an event, a timer, or a completion port. If a
/// second kind of thing wants to implement it, this trait moves to a lower
/// crate and this one depends on it; that move is planned rather than a
/// surprise, which is why it is said here.
pub trait Waitable {
    /// Borrows the queue's readiness as a waitable `HANDLE`.
    ///
    /// The event is created on the first call, so a consumer that only ever
    /// polls is charged for no kernel object.
    ///
    /// The borrow is deliberate: the event belongs to the queue and must not be
    /// closed. Use [`Waitable::doorbell_owned`] where ownership is required.
    ///
    /// # Errors
    ///
    /// Returns the error from `CreateEventW` on the first call.
    fn doorbell(&self) -> io::Result<BorrowedHandle<'_>>;

    /// A duplicate of [`Waitable::doorbell`] that the caller owns.
    ///
    /// The duplicate names the same event, so signalling reaches both. This is
    /// the form a `ThreadpoolWait` needs, since arming one takes ownership of
    /// its target.
    ///
    /// # Errors
    ///
    /// Returns the error from `CreateEventW` or `DuplicateHandle`.
    fn doorbell_owned(&self) -> io::Result<OwnedHandle>;

    /// Clears the doorbell and reports whether it is safe to wait on it.
    ///
    /// `true` means the queue had nothing to take *after* the doorbell was
    /// cleared, so any later push is guaranteed to signal and a wait cannot be
    /// missed. `false` means something arrived in the meantime: take it instead
    /// of waiting.
    ///
    /// **Waiting without arming is a permanent hang, not an occasional missed
    /// wakeup.** The full argument is in
    /// [D-9](../DESIGN-NOTES.md#d-9); the short form is that clearing must come
    /// before the emptiness check, which is the reverse of the order that reads
    /// naturally.
    ///
    /// # Errors
    ///
    /// Returns the error from `CreateEventW` on the first call.
    fn arm(&self) -> io::Result<bool>;
}

#[cfg(test)]
mod tests;
