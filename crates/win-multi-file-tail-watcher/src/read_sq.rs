// Copyright (c) 2026 Mike Grier
//! The reading layer's SQ: the bytes each poll announced, as chunks, and the entries that need no
//! bytes (TW-D-15). The crate writes it and the client drains it; it is public through
//! [`crate::reader`].

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

/// One entry of the reading layer's SQ, which the client drains. Open to further kinds, so match it
/// with a wildcard arm:
///
/// ```
/// # use win_multi_file_tail_watcher::reader::ReadEntry;
/// fn kind<T>(entry: &ReadEntry<T>) -> &'static str {
///     match entry {
///         ReadEntry::Data(_) => "data",
///         ReadEntry::Shrink(_) => "shrink",
///         ReadEntry::Terminal(_) => "terminal",
///         ReadEntry::PollDone => "poll done",
///         _ => "a kind this version does not know",
///     }
/// }
/// # assert_eq!(kind(&ReadEntry::<u8>::PollDone), "poll done");
/// ```
///
/// Without the wildcard arm it does not compile:
///
/// ```compile_fail,E0004
/// # use win_multi_file_tail_watcher::reader::ReadEntry;
/// fn kind<T>(entry: &ReadEntry<T>) -> &'static str {
///     match entry {
///         ReadEntry::Data(_) => "data",
///         ReadEntry::Shrink(_) => "shrink",
///         ReadEntry::Terminal(_) => "terminal",
///         ReadEntry::PollDone => "poll done",
///     }
/// }
/// ```
///
/// # Holding an entry
///
/// A [`ReadEntry::Data`] entry keeps a buffer out of the layer's pool until it is dropped, because
/// it holds a [`Chunk`]; see there for what that stops. The other kinds hold nothing but a clone of
/// the registration's value.
#[derive(Debug)]
#[non_exhaustive]
pub enum ReadEntry<T> {
    /// Bytes read from a registration's file.
    Data(Data<T>),
    /// A registration's file got shorter; nothing was read.
    Shrink(Shrink<T>),
    /// A registration can no longer be probed, and no longer will be (TW-D-13), or a read of it
    /// failed in a way that means its handle never will answer. For a failed read `last_known` is
    /// the offset the read began at, and the bytes from there are not read again.
    Terminal(Terminal<T>),
    /// Every read of a poll has been delivered: it follows the last read of its poll, and it is
    /// always delivered, including after a shutdown request. It ends the poll: the reader accepts
    /// another [`poll`](crate::reader::TailReader::poll) from the moment it can be seen, and refuses
    /// one before.
    PollDone,
}

/// Bytes read from a registration's file, delivered when their read completed: the chunks of one
/// file may arrive in any order, and each says where in the file its bytes are, so a client that
/// wants them in file order puts them back by [`range`](Self::range). A line often spans more than
/// one chunk, and joining them is the client's work.
///
/// Holding this holds its [`Chunk`], and so a buffer of the layer's pool: see there.
#[derive(Debug)]
pub struct Data<T> {
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
    pub fn value(&self) -> &T {
        &self.value
    }

    /// The file offsets of the bytes in [`chunk`](Self::chunk): its length is the chunk's.
    #[must_use]
    pub fn range(&self) -> Range<u64> {
        self.range.clone()
    }

    /// The bytes. Dropping them, with the entry or after [`into_parts`](Self::into_parts), gives
    /// their buffer back to the pool.
    pub fn chunk(&self) -> &Chunk {
        &self.chunk
    }

    /// The value, the range and the chunk, apart. The chunk still holds its buffer.
    #[must_use]
    pub fn into_parts(self) -> (T, Range<u64>, Chunk) {
        (self.value, self.range, self.chunk)
    }
}

/// A report that a registration's file is shorter than the length last reported for it. No bytes
/// are read for it.
///
/// The crate attaches no meaning to the change: a truncated, rotated or replaced file all look the
/// same, and the client decides what it was.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shrink<T> {
    /// A clone of the value the registration was made with.
    pub value: T,
    /// The length last reported for the registration.
    pub last_known: u64,
    /// The length now, which is less.
    pub current: u64,
}

/// The reading layer's SQ and its two doorbells, with the watcher's discipline (TW-D-6): the
/// layer writes and rings [`doorbell`](Self::doorbell), the client resets it before draining, and
/// the client rings [`resume_doorbell`](Self::resume_doorbell) when it has made room.
///
/// A client does not make one: the layer makes it and lends it.
///
/// # Holding up the layer
///
/// The SQ holds [`capacity`](Self::capacity) entries. A client that does not drain it stops the
/// layer reading once it is full: the entry it could not write is kept, nothing else is taken
/// meanwhile, and nothing is lost, but no more bytes are delivered and no poll-done after them.
/// Draining makes room; the layer goes on when the client rings
/// [`resume_doorbell`](Self::resume_doorbell).
pub struct ReadSq<T> {
    // Locked to clone a handle, which waits for nothing; a write then goes through the clone, so
    // the lock is not held while one is made.
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
    /// [`CreateError::Allocation`] when the memory for the queue's slots cannot be allocated.
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

    /// The entries in the SQ, oldest first, until there are none.
    ///
    /// Taking an entry makes room for another. Only one drain is live at a time: a second waits for
    /// the first to drop. It never waits for the layer's writing.
    pub fn drain(&self) -> ReadDrain<'_, T> {
        ReadDrain(self.reader.lock().unwrap_or_else(PoisonError::into_inner))
    }

    /// How many entries the SQ holds when full.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.reader
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .capacity()
    }

    /// The event the layer rings when entries are ready, the SQ is full, or a poll is done. It is
    /// rung until [`reset_doorbell`](Self::reset_doorbell), which releases every wait on it
    /// meanwhile.
    ///
    /// The client learns which of the three by reading the SQ. Reset first and then drain, so an
    /// entry written after the reset rings it again.
    pub fn doorbell(&self) -> BorrowedHandle<'_> {
        self.doorbell.handle()
    }

    /// Ring [`doorbell`](Self::doorbell).
    pub(crate) fn ring_doorbell(&self) {
        self.doorbell.ring();
    }

    /// Clear [`doorbell`](Self::doorbell). Does nothing if it is not rung.
    pub fn reset_doorbell(&self) {
        self.doorbell.reset();
    }

    /// The event the client rings, with [`ring_resume`](Self::ring_resume), to say it has made room
    /// in the SQ. The layer waits on it.
    pub fn resume_doorbell(&self) -> BorrowedHandle<'_> {
        self.resume.handle()
    }

    /// Ring [`resume_doorbell`](Self::resume_doorbell).
    pub fn ring_resume(&self) {
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
///
/// Holding it keeps any other drain out, so one held for long makes a second wait; it does not hold
/// up the layer's writing. It is held by the thread that took it.
pub struct ReadDrain<'a, T>(MutexGuard<'a, QueueReader<ReadEntry<T>>>);

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
