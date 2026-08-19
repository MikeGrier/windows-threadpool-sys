// Copyright (c) 2026 Mike Grier
//! Decoding of the `FILE_NOTIFY_INFORMATION` records that `ReadDirectoryChangesW`
//! writes into a completion buffer.
//!
//! The records form a chain: each carries a byte offset to the next and a
//! variable-length UTF-16 file name relative to the watched directory. The name
//! is not NUL-terminated and may hold arbitrary UTF-16 (unpaired surrogates,
//! `> MAX_PATH`), so it is preserved losslessly rather than validated as Unicode.
//!
//! Parsing is defensive: a truncated header, a name length that overruns the
//! buffer, or a `NextEntryOffset` that points outside it stops iteration instead
//! of reading out of bounds. Fields are read as little-endian byte pairs, so no
//! alignment of the caller's buffer is assumed.

use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;

use windows_sys::Win32::Storage::FileSystem::{
    FILE_ACTION_ADDED, FILE_ACTION_MODIFIED, FILE_ACTION_REMOVED, FILE_ACTION_RENAMED_NEW_NAME,
    FILE_ACTION_RENAMED_OLD_NAME,
};

/// The fixed portion of a `FILE_NOTIFY_INFORMATION` record: three `u32` fields
/// (`NextEntryOffset`, `Action`, `FileNameLength`) ahead of the name.
const HEADER_LEN: usize = 12;

/// A file name reported by `ReadDirectoryChangesW`, relative to the watched
/// directory, preserved as raw UTF-16 so no information is lost.
///
/// The kernel delivers names as UTF-16 that need not be well-formed Unicode
/// (unpaired surrogates are possible on NTFS) and that are not NUL-terminated.
/// [`RelativeName::as_wide`] exposes the raw units as the ground truth;
/// [`RelativeName::to_os_string`] and [`RelativeName::to_path_buf`] round-trip
/// them losslessly through the platform's WTF-8 `OsString` -- the standard
/// library guarantees `from_wide` followed by `encode_wide` returns the original
/// code units even for ill-formed UTF-16, so no fidelity is lost. (`OsStr::to_str`
/// and `to_string_lossy` remain lossy *views*, for display only.)
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct RelativeName {
    units: Box<[u16]>,
}

impl RelativeName {
    /// The raw UTF-16 code units, exactly as the kernel reported them.
    #[must_use]
    pub fn as_wide(&self) -> &[u16] {
        &self.units
    }

    /// The name as an `OsString`, round-tripping the raw UTF-16 losslessly.
    #[must_use]
    pub fn to_os_string(&self) -> OsString {
        OsString::from_wide(&self.units)
    }

    /// The name as a `PathBuf` (relative to the watched directory).
    #[must_use]
    pub fn to_path_buf(&self) -> PathBuf {
        PathBuf::from(self.to_os_string())
    }
}

impl std::fmt::Debug for RelativeName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Lossy rendering is for diagnostics only; the lossless forms are the
        // `as_wide` / `to_os_string` accessors.
        std::fmt::Debug::fmt(&self.to_os_string().to_string_lossy(), f)
    }
}

/// One decoded record: the raw `FILE_ACTION_*` code and the relative name.
///
/// Internal to the crate: the public surface is the typed [`Change`], which loses
/// no fidelity ([`ChangeKind::Unknown`] preserves an unrecognised action code).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RawChange {
    /// The raw `FILE_ACTION_*` value.
    pub action: u32,
    /// The reported name, relative to the watched directory.
    pub name: RelativeName,
}

/// Iterate the `FILE_NOTIFY_INFORMATION` chain in `buffer`.
///
/// `buffer` is the completion buffer truncated to the bytes the kernel returned.
/// Iteration stops at the first record that cannot be parsed cleanly -- a
/// truncated header, a name length that overruns the buffer, or a
/// `NextEntryOffset` that is unaligned, points into the current record, or falls
/// outside the buffer -- so a malformed or partial buffer can never cause an
/// out-of-bounds read. When iteration stops for such a reason the iterator sets
/// [`Records::malformed`]: the caller treats a malformed buffer as a desync
/// rather than a successful (possibly empty) batch, since a truncated buffer may
/// hide changes the client would otherwise believe it had seen. A clean end (a
/// final record with `NextEntryOffset == 0`) leaves `malformed` unset. A
/// zero-length buffer is the kernel's overflow signal and is handled by the
/// caller, not here.
#[must_use]
pub(crate) fn records(buffer: &[u8]) -> Records<'_> {
    Records {
        buffer,
        pos: Some(0),
        malformed: false,
    }
}

/// Iterator over the record chain, returned by [`records`].
pub(crate) struct Records<'a> {
    buffer: &'a [u8],
    pos: Option<usize>,
    /// Set once iteration stops because a record could not be parsed cleanly, as
    /// opposed to reaching a well-formed end of chain.
    malformed: bool,
}

impl Iterator for Records<'_> {
    type Item = RawChange;

    fn next(&mut self) -> Option<RawChange> {
        let pos = self.pos?;
        let Some(rec) = self.buffer.get(pos..) else {
            // A followed `NextEntryOffset` pointed past the buffer end.
            self.pos = None;
            self.malformed = true;
            return None;
        };
        if rec.len() < HEADER_LEN {
            // Too few bytes for a header: a truncated leading buffer, or a
            // followed offset that landed in a short tail.
            self.pos = None;
            self.malformed = true;
            return None;
        }

        let next_offset = read_u32(rec, 0) as usize;
        let action = read_u32(rec, 4);
        let name_len_bytes = read_u32(rec, 8) as usize;

        // The name must lie entirely within this record's span, or the buffer is
        // malformed and we stop rather than read past the end.
        let name_end = match HEADER_LEN.checked_add(name_len_bytes) {
            Some(end) if end <= rec.len() => end,
            _ => {
                self.pos = None;
                self.malformed = true;
                return None;
            }
        };
        let name = decode_utf16(&rec[HEADER_LEN..name_end]);

        // Advance to the next record, or finish. A well-formed `NextEntryOffset`
        // is a DWORD-aligned byte offset, measured from this record's start, that
        // clears this record's own header+name span. An offset that is unaligned,
        // points back into the current record (overlap), or would overflow is a
        // corrupt link: the current record is still yielded, but the chain is
        // marked malformed and iteration stops.
        self.pos = if next_offset == 0 {
            None
        } else if next_offset.is_multiple_of(4) && next_offset >= name_end {
            match pos.checked_add(next_offset) {
                Some(next) => Some(next),
                None => {
                    self.malformed = true;
                    None
                }
            }
        } else {
            self.malformed = true;
            None
        };

        Some(RawChange { action, name })
    }
}

/// Read a little-endian `u32` at `offset` within `rec`, which the caller has
/// ensured is at least `HEADER_LEN` bytes long.
fn read_u32(rec: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        rec[offset],
        rec[offset + 1],
        rec[offset + 2],
        rec[offset + 3],
    ])
}

/// Copy a UTF-16LE byte region into owned code units. A trailing odd byte (only
/// possible from a malformed length) is dropped rather than misread.
fn decode_utf16(bytes: &[u8]) -> RelativeName {
    let units: Box<[u16]> = bytes
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect();
    RelativeName { units }
}

/// The kind of change a `FILE_ACTION_*` code names.
///
/// `RenamedOldName` and `RenamedNewName` are kept distinct and are never joined
/// into a single rename event: a rename can straddle a completion boundary, and
/// pairing them is the client's decision. An action code this crate does not
/// recognise is preserved as [`ChangeKind::Unknown`] rather than dropped.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ChangeKind {
    /// `FILE_ACTION_ADDED`.
    Added,
    /// `FILE_ACTION_REMOVED`.
    Removed,
    /// `FILE_ACTION_MODIFIED`.
    Modified,
    /// `FILE_ACTION_RENAMED_OLD_NAME`: the old name of a rename within the watched
    /// tree; the matching new name follows as a separate record.
    RenamedOldName,
    /// `FILE_ACTION_RENAMED_NEW_NAME`: the new name of a rename within the watched
    /// tree.
    RenamedNewName,
    /// An action code this crate does not recognise, preserved verbatim.
    Unknown(u32),
}

impl ChangeKind {
    /// Map a raw `FILE_ACTION_*` code to its kind, preserving an unrecognised
    /// code in [`ChangeKind::Unknown`].
    fn from_action(action: u32) -> ChangeKind {
        match action {
            FILE_ACTION_ADDED => ChangeKind::Added,
            FILE_ACTION_REMOVED => ChangeKind::Removed,
            FILE_ACTION_MODIFIED => ChangeKind::Modified,
            FILE_ACTION_RENAMED_OLD_NAME => ChangeKind::RenamedOldName,
            FILE_ACTION_RENAMED_NEW_NAME => ChangeKind::RenamedNewName,
            other => ChangeKind::Unknown(other),
        }
    }
}

/// One decoded change: its [`ChangeKind`] and the [`RelativeName`] it applies to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    /// What kind of change occurred.
    pub kind: ChangeKind,
    /// The affected name, relative to the watched directory.
    pub name: RelativeName,
}

/// Why a [`DecodedBatch::Desync`] -- a "you may have missed changes, re-scan"
/// signal -- was raised.
///
/// The cause is advisory: the client's response is the same in every case (a
/// re-scan). It exists so a client can diagnose *how* it fell behind.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DesyncCause {
    /// The kernel change buffer overflowed (a zero-byte completion): changes were
    /// dropped by the OS before this crate observed them. A completion buffer the
    /// crate could not fully parse (truncated, overrunning, or a corrupt record
    /// chain) is reported with this cause as well: it too means changes may be
    /// missing, and the client's response -- re-scan -- is identical.
    Overflow,
    /// The client's bounded notification queue was full, so a batch was dropped
    /// rather than block the monitor. Produced by the delivery layer.
    QueueFull,
    /// The watch is in coarse mode (`FindFirstChangeNotification`), which reports
    /// only that *something* changed, never what. Produced by the coarse watcher.
    Coarse,
    /// The watch was re-established after an outage; changes during the gap were
    /// lost. Produced by the fault-recovery path.
    Reestablished,
}

/// The result of decoding one `ReadDirectoryChangesW` completion.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DecodedBatch {
    /// The changes the completion carried, in the order the kernel reported them.
    Changes(Vec<Change>),
    /// The completion signalled lost changes rather than carrying any; the client
    /// is told to re-scan.
    Desync(DesyncCause),
}

/// Decode one `ReadDirectoryChangesW` completion buffer into a batch.
///
/// A zero-byte completion is the kernel's overflow signal and decodes to
/// [`DecodedBatch::Desync`] with [`DesyncCause::Overflow`]. A non-empty buffer
/// the crate cannot fully parse -- a truncated or overrunning record, or a
/// corrupt `NextEntryOffset` chain -- is likewise a [`DesyncCause::Overflow`]
/// desync rather than a successful (possibly partial) batch, so a client is
/// never falsely told it stayed synchronized. Otherwise the record chain (see
/// the module docs for the defensive parsing) is decoded into
/// [`DecodedBatch::Changes`].
#[must_use]
pub fn decode_batch(buffer: &[u8]) -> DecodedBatch {
    if buffer.is_empty() {
        return DecodedBatch::Desync(DesyncCause::Overflow);
    }
    let mut iter = records(buffer);
    let changes: Vec<Change> = iter
        .by_ref()
        .map(|raw| Change {
            kind: ChangeKind::from_action(raw.action),
            name: raw.name,
        })
        .collect();
    if iter.malformed {
        return DecodedBatch::Desync(DesyncCause::Overflow);
    }
    DecodedBatch::Changes(changes)
}

#[cfg(test)]
mod tests;
