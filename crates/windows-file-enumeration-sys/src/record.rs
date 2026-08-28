// Copyright (c) 2026 Mike Grier
//! Parsing one `FILE_ID_EXTD_DIR_INFO` batch, one record at a time.
//!
//! # Validate, then trust
//!
//! Every precondition below is checked before the field it protects is read:
//! an alignment failure never leads to an out-of-bounds read, a truncated
//! record is never asked for a name, and a name is never trusted before its
//! bounds are known to fit the batch. Once a record passes every check its
//! fields are read once and moved into a [`ParsedRecord`] -- there is no second
//! pass that re-reads the buffer with fewer guards.
//!
//! # The fixed layout
//!
//! `FILE_ID_EXTD_DIR_INFO` declares no padding: `FileIndex` (never read --
//! Windows documents it as undefined on NTFS) sits between the two leading
//! `u32`s and the first `i64`, and every multi-byte field before the trailing
//! name is naturally aligned by the fields that precede it. [`FIXED_FIELDS_LEN`]
//! is the byte offset of that name, and so the smallest a record can be.
//!
//! # Why fields are read from a byte slice rather than a pointer cast
//!
//! The batch is already known to be 8-byte aligned as a whole (see
//! [`buffer`](crate::buffer)), but a record's *content* -- its name -- is not
//! fixed width, so later records in the same batch are not guaranteed to sit on
//! any particular alignment beyond the 8-byte boundary the API itself
//! maintains between records. Reading every field with `from_ne_bytes` over a
//! byte range needs no alignment of its own and is exactly as fast as a
//! pointer read would be.
//!
//! # `.` and `..`
//!
//! Every directory-information query reports the directory itself and its
//! parent as ordinary records ahead of real entries. [`ParsedRecord::is_dot_or_dotdot`]
//! is what lets the engine drop them before a predicate ever sees them, rather
//! than asking every query-by-example clause to know that convention exists.

use wtf_string::Wtf16String;

use crate::entry::{EntryFields, FileIdentity};
use crate::error::MalformedRecord;
use crate::request::RECORD_ALIGNMENT;
use crate::timestamp::WindowsFileTimestamp;

/// Byte offsets of a record's fixed fields, relative to its own start.
///
/// Named so a validation failure or a field read can be reasoned about without
/// recounting bytes against the Win32 struct definition.
mod field {
    pub(super) const NEXT_ENTRY_OFFSET: usize = 0;
    // FileIndex sits at 4..8. It is never read (see the module doc), but its
    // four bytes are still part of the fixed layout every offset below is
    // computed from.
    pub(super) const CREATION_TIME: usize = 8;
    pub(super) const LAST_ACCESS_TIME: usize = 16;
    pub(super) const LAST_WRITE_TIME: usize = 24;
    pub(super) const CHANGE_TIME: usize = 32;
    pub(super) const END_OF_FILE: usize = 40;
    pub(super) const ALLOCATION_SIZE: usize = 48;
    pub(super) const FILE_ATTRIBUTES: usize = 56;
    pub(super) const FILE_NAME_LENGTH: usize = 60;
    pub(super) const EA_SIZE: usize = 64;
    pub(super) const REPARSE_POINT_TAG: usize = 68;
    pub(super) const FILE_ID: usize = 72;
}

/// The byte length of every fixed field, and so the offset of the name that
/// follows them.
const FIXED_FIELDS_LEN: usize = 88;

/// `'.'` as a native code unit -- what a directory's self- and parent-entry
/// names are made of.
const DOT: u16 = 0x002E;

/// One record's fields, decoded from a batch.
///
/// Everything is owned rather than borrowed from the batch: the buffer this
/// was read from is reused for the very next refill, so a record that outlives
/// its quantum -- because delivery found no room and the cursor stayed put --
/// must not still be looking at it.
#[derive(Debug)]
pub(crate) struct ParsedRecord {
    name: Wtf16String,
    attributes: u32,
    logical_size: u64,
    allocation_size: u64,
    extended_attribute_size: u32,
    creation_time: WindowsFileTimestamp,
    last_access_time: WindowsFileTimestamp,
    last_write_time: WindowsFileTimestamp,
    change_time: WindowsFileTimestamp,
    reparse_tag: u32,
    file_id: [u8; 16],
}

impl ParsedRecord {
    /// Whether this record names the directory itself or its parent.
    #[must_use]
    pub(crate) fn is_dot_or_dotdot(&self) -> bool {
        matches!(self.name.as_units(), [DOT] | [DOT, DOT])
    }

    /// Consume the record, stamping its identity with whatever volume serial
    /// the engine obtained.
    #[must_use]
    pub(crate) fn into_fields(self, volume_serial: Option<u64>) -> EntryFields {
        EntryFields {
            name: self.name,
            attributes: self.attributes,
            logical_size: self.logical_size,
            allocation_size: self.allocation_size,
            extended_attribute_size: self.extended_attribute_size,
            creation_time: self.creation_time,
            last_access_time: self.last_access_time,
            last_write_time: self.last_write_time,
            change_time: self.change_time,
            reparse_tag: self.reparse_tag,
            identity: FileIdentity::new(self.file_id, volume_serial),
        }
    }
}

fn read_u32(bytes: &[u8], at: usize) -> u32 {
    u32::from_ne_bytes(
        bytes[at..at + size_of::<u32>()]
            .try_into()
            .expect("caller validated the fixed-field extent"),
    )
}

fn read_i64(bytes: &[u8], at: usize) -> i64 {
    i64::from_ne_bytes(
        bytes[at..at + size_of::<i64>()]
            .try_into()
            .expect("caller validated the fixed-field extent"),
    )
}

/// Decode a name from its raw little-endian bytes, one code unit at a time.
///
/// Reading pairs rather than reinterpreting the range as `[u16]` needs no
/// alignment of its own, which matters because a name's start is only ever
/// known to be even, not 8-byte aligned.
fn decode_name(bytes: &[u8]) -> Wtf16String {
    let units: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_ne_bytes(*pair))
        .collect();
    Wtf16String::from_units(&units)
}

/// Parse the record starting at `record_start` within `bytes`.
///
/// On success, returns the record and -- when another one follows it in this
/// batch -- the absolute offset that record starts at. `None` means this was
/// the batch's last record, exactly mirroring a `NextEntryOffset` of zero.
///
/// # Errors
///
/// Returns the specific [`MalformedRecord`] that made the record unsafe to
/// trust, without having read a single field beyond what validation itself
/// required.
pub(crate) fn parse_record(
    bytes: &[u8],
    record_start: usize,
) -> Result<(ParsedRecord, Option<usize>), MalformedRecord> {
    if !record_start.is_multiple_of(RECORD_ALIGNMENT) {
        return Err(MalformedRecord::Alignment);
    }
    let remaining = bytes.len() - record_start;
    if remaining < FIXED_FIELDS_LEN {
        return Err(MalformedRecord::TruncatedFixedFields);
    }

    let next_entry_offset = read_u32(bytes, record_start + field::NEXT_ENTRY_OFFSET);

    let name_length = read_u32(bytes, record_start + field::FILE_NAME_LENGTH);
    if !name_length.is_multiple_of(2) {
        return Err(MalformedRecord::OddNameLength);
    }
    let name_start = record_start + FIXED_FIELDS_LEN;
    let name_end = name_start
        .checked_add(name_length as usize)
        .filter(|&end| end <= bytes.len())
        .ok_or(MalformedRecord::NameOutOfBounds)?;

    // Validated last, because it is only meaningful once `name_end` is known:
    // a record's own extent -- fixed fields plus name -- is exactly what its
    // next-entry offset must not land inside of.
    let next_record_start = if next_entry_offset == 0 {
        None
    } else {
        let candidate = record_start
            .checked_add(next_entry_offset as usize)
            .filter(|&candidate| candidate >= name_end && candidate <= bytes.len())
            .ok_or(MalformedRecord::NextEntryOffset)?;
        Some(candidate)
    };

    let end_of_file = read_i64(bytes, record_start + field::END_OF_FILE);
    let allocation_size = read_i64(bytes, record_start + field::ALLOCATION_SIZE);
    let logical_size = u64::try_from(end_of_file).map_err(|_| MalformedRecord::NegativeSize)?;
    let allocation_size =
        u64::try_from(allocation_size).map_err(|_| MalformedRecord::NegativeSize)?;

    let creation_time = read_i64(bytes, record_start + field::CREATION_TIME);
    let last_access_time = read_i64(bytes, record_start + field::LAST_ACCESS_TIME);
    let last_write_time = read_i64(bytes, record_start + field::LAST_WRITE_TIME);
    let change_time = read_i64(bytes, record_start + field::CHANGE_TIME);
    let attributes = read_u32(bytes, record_start + field::FILE_ATTRIBUTES);
    let extended_attribute_size = read_u32(bytes, record_start + field::EA_SIZE);
    let reparse_tag = read_u32(bytes, record_start + field::REPARSE_POINT_TAG);

    let mut file_id = [0u8; 16];
    file_id
        .copy_from_slice(&bytes[record_start + field::FILE_ID..record_start + field::FILE_ID + 16]);

    let name = decode_name(&bytes[name_start..name_end]);

    let record = ParsedRecord {
        name,
        attributes,
        logical_size,
        allocation_size,
        extended_attribute_size,
        creation_time: WindowsFileTimestamp::from_ticks(creation_time),
        last_access_time: WindowsFileTimestamp::from_ticks(last_access_time),
        last_write_time: WindowsFileTimestamp::from_ticks(last_write_time),
        change_time: WindowsFileTimestamp::from_ticks(change_time),
        reparse_tag,
        file_id,
    };
    Ok((record, next_record_start))
}

#[cfg(test)]
mod tests;
