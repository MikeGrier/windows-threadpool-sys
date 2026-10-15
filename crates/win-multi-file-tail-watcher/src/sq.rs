// Copyright (c) 2026 Mike Grier
//! The submission queue (SQ): what the watcher writes and the caller drains.

use std::fmt;
use std::sync::MutexGuard;

use windows_waitable_queues::{CapacityError, PushError, slotwise_mpsc};

#[cfg(test)]
mod tests;

/// One entry of the SQ.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SqEntry<T> {
    /// A registration's length changed.
    Attention(Attention<T>),
    /// A poll has written everything it had for this beat.
    PollDone,
}

/// A report that a registration's length differs from the one last reported for it.
///
/// The crate attaches no meaning to the change: it is either direction, and the receiver
/// interprets the pair.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attention<T> {
    /// A clone of the value the registration was made with.
    pub value: T,
    /// The length last reported for the registration.
    pub last_known: u64,
    /// The length now.
    pub current: u64,
}

/// A capacity the SQ cannot have.
///
/// The capacity is the number of entries the SQ holds, and must be a power of two of at least two.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SqCapacityError {
    /// The capacity asked for.
    pub requested: usize,
    /// The smallest capacity the SQ accepts.
    pub min_valid: usize,
    /// The largest capacity the SQ accepts.
    pub max_valid: usize,
}

impl fmt::Display for SqCapacityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "SQ capacity {} is not a power of two from {} to {}",
            self.requested, self.min_valid, self.max_valid
        )
    }
}

impl std::error::Error for SqCapacityError {}

impl From<CapacityError> for SqCapacityError {
    fn from(error: CapacityError) -> Self {
        Self {
            requested: error.requested(),
            min_valid: error.min_valid(),
            max_valid: error.max_valid(),
        }
    }
}

/// Why an entry was not written. The entry comes back with it.
#[derive(Debug, PartialEq, Eq)]
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "the report pass (TW-2.7) writes through it")
)]
pub(crate) enum SqPushError<T> {
    /// Every slot holds an entry the caller has not drained.
    Full(SqEntry<T>),
    /// The reading side is gone.
    Closed(SqEntry<T>),
}

/// The writing side of the SQ. One handle per writing thread: a handle is cloned, not shared.
#[derive(Debug)]
pub(crate) struct SqWriter<T>(slotwise_mpsc::Producer<SqEntry<T>>);

impl<T> Clone for SqWriter<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<T> SqWriter<T> {
    /// Write `entry` without waiting.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "the report pass (TW-2.7) writes through it")
    )]
    pub(crate) fn push(&self, entry: SqEntry<T>) -> Result<(), SqPushError<T>> {
        self.0.push(entry).map_err(|error| match error {
            PushError::Full(entry) => SqPushError::Full(entry),
            // `Disconnected`, and any refusal the queue's crate adds: none of them is retryable.
            other => SqPushError::Closed(other.into_inner()),
        })
    }
}

/// The reading side of the SQ.
#[derive(Debug)]
pub(crate) struct SqReader<T>(slotwise_mpsc::Consumer<SqEntry<T>>);

impl<T> SqReader<T> {
    /// The next entry, or `None` when there is none now, or when no writer is left to add one.
    pub(crate) fn pop(&self) -> Option<SqEntry<T>> {
        self.0.pop().ok()
    }

    pub(crate) fn capacity(&self) -> usize {
        self.0.capacity()
    }
}

/// A new SQ holding `capacity` entries.
pub(crate) fn queue<T>(capacity: usize) -> Result<(SqWriter<T>, SqReader<T>), SqCapacityError> {
    let (writer, reader) = slotwise_mpsc::bounded(capacity)?;
    Ok((SqWriter(writer), SqReader(reader)))
}

/// The entries the SQ holds, taken in the order they were written, until there are none.
///
/// Holding it keeps any other drain out; it does not hold up the writers.
pub struct SqDrain<'a, T>(pub(crate) MutexGuard<'a, SqReader<T>>);

impl<T> Iterator for SqDrain<'_, T> {
    type Item = SqEntry<T>;

    fn next(&mut self) -> Option<SqEntry<T>> {
        self.0.pop()
    }
}

impl<T> fmt::Debug for SqDrain<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SqDrain").finish_non_exhaustive()
    }
}
