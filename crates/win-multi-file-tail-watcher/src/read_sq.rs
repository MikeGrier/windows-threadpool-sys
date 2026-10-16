// Copyright (c) 2026 Mike Grier
//! The reading layer's SQ: the bytes each poll announced, as chunks, and the entries that need no
//! bytes (TW-D-15). Its working names are placeholders until the layer's own is chosen.
#![cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the reading engine (TW-5.5) writes it and the reading layer hands it out"
    )
)]

use std::fmt;
use std::ops::Range;
use std::os::windows::io::BorrowedHandle;
use std::sync::{Mutex, MutexGuard, PoisonError};

use crate::CreateError;
use crate::doorbell::Doorbell;
use crate::pool::Chunk;
use crate::sq::{PushFailure, QueueReader, QueueWriter, Terminal, queue_of};

#[cfg(test)]
mod tests;

/// One entry of the reading layer's SQ. Open to further kinds: match it with a wildcard arm.
#[derive(Debug)]
#[non_exhaustive]
pub(crate) enum ReadEntry<T> {
    /// Bytes read from a registration's file.
    Data(Data<T>),
    /// A registration's file got shorter; nothing was read.
    Shrink(Shrink<T>),
    /// A registration can no longer be probed, and no longer will be (TW-D-13).
    Terminal(Terminal<T>),
    /// Every read of a poll has been delivered.
    PollDone,
}

/// Bytes read from a registration's file, delivered when their read completed: the chunks of one
/// file may arrive in any order, and each says where in the file its bytes are.
#[derive(Debug)]
pub(crate) struct Data<T> {
    value: T,
    range: Range<u64>,
    chunk: Chunk,
}

impl<T> Data<T> {
    /// The bytes of `chunk`, read from the file at `offset`. The range is computed from the chunk,
    /// so it always covers exactly the bytes delivered.
    ///
    /// # Panics
    ///
    /// When the range would end past `u64::MAX`, which no file offset reaches.
    pub(crate) fn new(value: T, offset: u64, chunk: Chunk) -> Self {
        let len = u64::try_from(chunk.len()).expect("a chunk is shorter than u64::MAX");
        let end = offset
            .checked_add(len)
            .expect("a file range ends before u64::MAX");
        Self {
            value,
            range: offset..end,
            chunk,
        }
    }

    /// A clone of the value the registration was made with.
    pub(crate) fn value(&self) -> &T {
        &self.value
    }

    /// The file offsets of the bytes in [`chunk`](Self::chunk).
    pub(crate) fn range(&self) -> Range<u64> {
        self.range.clone()
    }

    /// The bytes. Dropping them, with the entry or after [`into_parts`](Self::into_parts), gives
    /// their buffer back to the pool.
    pub(crate) fn chunk(&self) -> &Chunk {
        &self.chunk
    }

    /// The value, the range and the chunk, apart.
    pub(crate) fn into_parts(self) -> (T, Range<u64>, Chunk) {
        (self.value, self.range, self.chunk)
    }
}

/// A report that a registration's file is shorter than the length last reported for it. No bytes
/// are read for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Shrink<T> {
    /// A clone of the value the registration was made with.
    pub(crate) value: T,
    /// The length last reported for the registration.
    pub(crate) last_known: u64,
    /// The length now, which is less.
    pub(crate) current: u64,
}

/// The reading layer's SQ and its two doorbells, with the watcher's discipline (TW-D-6): the
/// layer writes and rings [`doorbell`](Self::doorbell), the client resets it before draining, and
/// the client rings [`resume_doorbell`](Self::resume_doorbell) when it has made room.
pub(crate) struct ReadSq<T> {
    // Held only to hand out clones, so no writer waits on this lock to write.
    writer: Mutex<QueueWriter<ReadEntry<T>>>,
    reader: Mutex<QueueReader<ReadEntry<T>>>,
    doorbell: Doorbell,
    resume: Doorbell,
}

impl<T> ReadSq<T> {
    /// An SQ holding `capacity` entries, by the same rule as the watcher's.
    ///
    /// # Errors
    ///
    /// [`CreateError::Capacity`] unless `capacity` is a power of two of at least two.
    /// [`CreateError::Io`] when a doorbell cannot be created.
    pub(crate) fn new(capacity: usize) -> Result<Self, CreateError> {
        let (writer, reader) = queue_of(capacity)?;
        Ok(Self {
            writer: Mutex::new(writer),
            reader: Mutex::new(reader),
            doorbell: Doorbell::new()?,
            resume: Doorbell::new()?,
        })
    }

    /// A writing handle, for one writing thread.
    pub(crate) fn writer(&self) -> QueueWriter<ReadEntry<T>> {
        self.writer
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Write `entry` without waiting; a full SQ hands it back.
    pub(crate) fn push(&self, entry: ReadEntry<T>) -> Result<(), PushFailure<ReadEntry<T>>> {
        self.writer().push(entry)
    }

    /// The entries in the SQ, oldest first, until there are none. One drain is live at a time.
    pub(crate) fn drain(&self) -> ReadDrain<'_, T> {
        ReadDrain(self.reader.lock().unwrap_or_else(PoisonError::into_inner))
    }

    /// How many entries the SQ holds when full.
    pub(crate) fn capacity(&self) -> usize {
        self.reader
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .capacity()
    }

    /// The event the layer rings when entries are ready, the SQ is full, or a poll is done.
    pub(crate) fn doorbell(&self) -> BorrowedHandle<'_> {
        self.doorbell.handle()
    }

    /// Ring [`doorbell`](Self::doorbell).
    pub(crate) fn ring_doorbell(&self) {
        self.doorbell.ring();
    }

    /// Clear [`doorbell`](Self::doorbell).
    pub(crate) fn reset_doorbell(&self) {
        self.doorbell.reset();
    }

    /// The event the client rings when it has made room.
    pub(crate) fn resume_doorbell(&self) -> BorrowedHandle<'_> {
        self.resume.handle()
    }

    /// Ring [`resume_doorbell`](Self::resume_doorbell).
    pub(crate) fn ring_resume(&self) {
        self.resume.ring();
    }

    /// Clear [`resume_doorbell`](Self::resume_doorbell).
    pub(crate) fn reset_resume(&self) {
        self.resume.reset();
    }
}

impl<T> fmt::Debug for ReadSq<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReadSq").finish_non_exhaustive()
    }
}

/// The entries the reading layer's SQ holds, in the order they were written, until there are none.
pub(crate) struct ReadDrain<'a, T>(MutexGuard<'a, QueueReader<ReadEntry<T>>>);

impl<T> Iterator for ReadDrain<'_, T> {
    type Item = ReadEntry<T>;

    fn next(&mut self) -> Option<ReadEntry<T>> {
        self.0.pop()
    }
}

impl<T> fmt::Debug for ReadDrain<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReadDrain").finish_non_exhaustive()
    }
}
