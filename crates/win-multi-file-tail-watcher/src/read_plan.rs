// Copyright (c) 2026 Mike Grier
//! What the reading layer does with each of the watcher's entries (TW-D-15): the bytes an
//! attention announced, as reads of one buffer each, or an entry delivered with no read.
#![cfg_attr(
    not(test),
    expect(dead_code, reason = "the reading engine (TW-5.5) plans its reads here")
)]

use std::iter::FusedIterator;
use std::num::NonZeroU32;
use std::ops::Range;

use crate::read_sq::{ReadEntry, Shrink};
use crate::sq::{Attention, SqEntry};

#[cfg(test)]
mod tests;

/// What one of the watcher's entries asks of the reading layer.
#[derive(Debug)]
pub(crate) enum Plan<T> {
    /// The bytes a registration's file gained, to be read.
    Reads(Reads<T>),
    /// An entry for the client that needs no read.
    Deliver(ReadEntry<T>),
    /// The end of a poll, delivered after every read the poll planned.
    PollDone,
}

/// The plan for `entry`, reading at most `buffer_len` bytes at a time.
pub(crate) fn plan<T>(entry: SqEntry<T>, buffer_len: NonZeroU32) -> Plan<T> {
    match entry {
        SqEntry::Attention(Attention {
            value,
            last_known,
            current,
        }) if current < last_known => Plan::Deliver(ReadEntry::Shrink(Shrink {
            value,
            last_known,
            current,
        })),
        SqEntry::Attention(Attention {
            value,
            last_known,
            current,
        }) => Plan::Reads(Reads {
            value,
            remaining: last_known..current,
            buffer_len,
        }),
        SqEntry::Terminal(terminal) => Plan::Deliver(ReadEntry::Terminal(terminal)),
        SqEntry::PollDone => Plan::PollDone,
    }
}

/// One read: `len` bytes of the file from `offset`, into one buffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PlannedRead {
    pub(crate) offset: u64,
    pub(crate) len: u32,
}

impl PlannedRead {
    /// The file offsets the read covers.
    pub(crate) fn range(self) -> Range<u64> {
        self.offset..self.offset + u64::from(self.len)
    }
}

/// The reads still to submit for one attention, in file order. Computed as they are taken, so
/// any range costs the same to plan.
#[derive(Debug)]
pub(crate) struct Reads<T> {
    value: T,
    remaining: Range<u64>,
    buffer_len: NonZeroU32,
}

impl<T> Reads<T> {
    /// The value the registration was made with.
    pub(crate) fn value(&self) -> &T {
        &self.value
    }

    /// The file offsets not yet taken as reads.
    pub(crate) fn remaining(&self) -> Range<u64> {
        self.remaining.clone()
    }

    /// The next read, left in the plan until [`next`](Iterator::next) takes it: a read that cannot
    /// be submitted now stays planned.
    pub(crate) fn front(&self) -> Option<PlannedRead> {
        if self.remaining.is_empty() {
            return None;
        }
        let buffer_len = self.buffer_len.get();
        let len = u32::try_from(self.remaining.end - self.remaining.start)
            .map_or(buffer_len, |left| left.min(buffer_len));
        Some(PlannedRead {
            offset: self.remaining.start,
            len,
        })
    }
}

impl<T> Iterator for Reads<T> {
    type Item = PlannedRead;

    fn next(&mut self) -> Option<PlannedRead> {
        let read = self.front()?;
        self.remaining.start = read.range().end;
        Some(read)
    }
}

impl<T> FusedIterator for Reads<T> {}
