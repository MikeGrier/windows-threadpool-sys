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
//! [D-2](../../DESIGN-NOTES.md#d-2).
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
//! themselves waited for `mpsc` to exist to be checked against. That is
//! [D-3](../../DESIGN-NOTES.md#d-3), and the check it demands is not rhetorical:
//! `mpsc` is a lock-free multi-producer array queue with no structural
//! resemblance to `spsc` beyond its interface, so a signature that fitted only
//! the first shape would have failed here rather than in a consumer's code.
//!
//! # The name a trait shares with a handle
//!
//! [`Producer`] and [`Consumer`] are also the names of the concrete handles in
//! [`spsc`](crate::spsc) and [`mpsc`](crate::mpsc). That is deliberate: the
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
    /// [D-9](../../DESIGN-NOTES.md#d-9); the short form is that clearing must come
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
