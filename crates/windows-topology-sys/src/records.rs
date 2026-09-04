// Copyright (c) 2026 Mike Grier
//! Walking a `Size`-chained record list.
//!
//! Both of this crate's Win32 enumerations return the same shape: a buffer of
//! consecutive, variable-length records, each declaring its own byte length in
//! a `Size` field at a fixed offset. `GetLogicalProcessorInformationEx` and
//! `GetSystemCpuSetInformation` differ in where that field sits and in what
//! the rest of the record holds, and in nothing else about the traversal.
//!
//! So the traversal is written once, here. Per
//! [D-24](../DESIGN-NOTES.md#d-24) this is **not** a trust boundary and not a
//! validation pass: the operating system is trusted for the structural
//! validity of a buffer it just wrote. Bounds appear because they are how a
//! walk knows where a record ends -- they are load-bearing for decoding, not a
//! guard against a hostile kernel.
//!
//! Two properties follow from that, and they are the reason this module
//! exists rather than each decoder open-coding the loop:
//!
//! - **A record view cannot read past itself.** [`Record::read`] is bounded by
//!   the record's own `Size`, so a trailing array sized by a count *from* the
//!   record cannot reach beyond it however large that count claims to be.
//! - **Nothing panics, and nothing is silently dropped.** A record that does
//!   not fit ends the walk and is reported through [`RecordWalk::anomaly`],
//!   for the caller to record alongside the data it did decode.

use crate::EnumerationAnomaly;
use crate::observation::Source;

/// One record, bounded by the `Size` it declared.
///
/// Reads through this type cannot leave the record, which is what keeps a
/// trailing array honest: its length comes from inside the record, and the
/// bytes it would span are checked against the record's own extent.
#[derive(Clone, Copy)]
pub(crate) struct Record {
    base: *const u8,
    size: usize,
    offset: usize,
}

impl Record {
    /// This record's declared length in bytes.
    ///
    /// Used when reporting a record too short for the body its relationship
    /// names: the declared length is half of what makes that anomaly legible.
    pub(crate) fn size(self) -> usize {
        self.size
    }

    /// This record's byte offset within the buffer, for reporting where an
    /// anomaly was found.
    pub(crate) fn offset(self) -> usize {
        self.offset
    }

    /// Read a `T` at `offset` within this record.
    ///
    /// Returns `None` when the read would leave the record, which is the
    /// bound that makes a count-driven trailing array safe to follow.
    ///
    /// # Safety
    ///
    /// The record must address `self.size` initialized bytes, which
    /// [`RecordWalk`] establishes before yielding it.
    pub(crate) unsafe fn read<T: Copy>(self, offset: usize) -> Option<T> {
        let end = offset.checked_add(size_of::<T>())?;
        if end > self.size {
            return None;
        }
        // SAFETY: the bound above proves `[offset, offset + size_of::<T>())`
        // lies within the record, whose bytes the caller guaranteed are
        // initialized. `read_unaligned` because a record's fields are laid out
        // by the API, not by Rust.
        Some(unsafe { self.base.add(offset).cast::<T>().read_unaligned() })
    }

    /// Read up to `count` consecutive `T` starting at `offset`, stopping at
    /// the record's end.
    ///
    /// The second element of the pair is `false` when the record was too short
    /// to hold all `count` entries -- the caller decides whether a short read
    /// is worth recording, since for some records a count of zero legitimately
    /// means one legacy entry.
    ///
    /// # Safety
    ///
    /// As [`Record::read`].
    pub(crate) unsafe fn read_array<T: Copy>(self, offset: usize, count: usize) -> (Vec<T>, bool) {
        let mut out = Vec::with_capacity(count.min(self.size / size_of::<T>().max(1)));
        for index in 0..count {
            let Some(at) = index
                .checked_mul(size_of::<T>())
                .and_then(|o| o.checked_add(offset))
            else {
                return (out, false);
            };
            // SAFETY: forwarded from the caller; the read is bounded by the
            // record and yields `None` rather than reaching past it.
            match unsafe { self.read::<T>(at) } {
                Some(value) => out.push(value),
                None => return (out, false),
            }
        }
        (out, true)
    }
}

/// An iterator over a `Size`-chained record list.
///
/// Yields each record that fits, then stops. When it stopped because a record
/// did not fit, [`RecordWalk::anomaly`] says so; when it stopped because the
/// buffer ran out cleanly, that is `None`.
pub(crate) struct RecordWalk {
    base: *const u8,
    length: usize,
    offset: usize,
    size_offset: usize,
    minimum: usize,
    source: Source,
    anomaly: Option<EnumerationAnomaly>,
}

impl RecordWalk {
    /// # Safety
    ///
    /// `base` must address `length` initialized bytes.
    pub(crate) unsafe fn new(
        base: *const u8,
        length: u32,
        size_offset: usize,
        minimum: usize,
        source: Source,
    ) -> Self {
        Self {
            base,
            length: length as usize,
            offset: 0,
            size_offset,
            minimum,
            source,
            anomaly: None,
        }
    }

    /// The record that ended the walk early, if one did.
    pub(crate) fn anomaly(&self) -> Option<EnumerationAnomaly> {
        self.anomaly.clone()
    }
}

impl Iterator for RecordWalk {
    type Item = Record;

    fn next(&mut self) -> Option<Record> {
        if self.anomaly.is_some() {
            return None;
        }
        // A record must at least carry its own `Size` field to be walkable at
        // all. Running out here is the ordinary end of the buffer, not an
        // anomaly, when nothing is left over.
        let header_end = self.offset + self.size_offset + size_of::<u32>();
        if header_end > self.length {
            if self.offset < self.length {
                self.anomaly = Some(EnumerationAnomaly::trailing_bytes(
                    self.source,
                    self.offset,
                    self.length - self.offset,
                ));
            }
            return None;
        }

        // SAFETY: the bound above proves the `Size` field is within the buffer
        // the caller guaranteed is initialized.
        let size = unsafe {
            self.base
                .add(self.offset + self.size_offset)
                .cast::<u32>()
                .read_unaligned()
        } as usize;

        if size < self.minimum {
            self.anomaly = Some(EnumerationAnomaly::undersized(
                self.source,
                self.offset,
                size,
                self.minimum,
            ));
            return None;
        }
        let Some(end) = self.offset.checked_add(size) else {
            self.anomaly = Some(EnumerationAnomaly::overruns(
                self.source,
                self.offset,
                size,
                self.length - self.offset,
            ));
            return None;
        };
        if end > self.length {
            self.anomaly = Some(EnumerationAnomaly::overruns(
                self.source,
                self.offset,
                size,
                self.length - self.offset,
            ));
            return None;
        }

        // SAFETY: `[offset, offset + size)` is within the buffer.
        let record = Record {
            base: unsafe { self.base.add(self.offset) },
            size,
            offset: self.offset,
        };
        self.offset = end;
        Some(record)
    }
}

#[cfg(test)]
mod tests;
