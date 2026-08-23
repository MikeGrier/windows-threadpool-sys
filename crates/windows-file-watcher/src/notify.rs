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
use std::path::PathBuf;

use wtf_string::{Wtf16Str, Wtf16String};

use windows_sys::Win32::Storage::FileSystem::{
    FILE_ACTION_ADDED, FILE_ACTION_MODIFIED, FILE_ACTION_REMOVED, FILE_ACTION_RENAMED_NEW_NAME,
    FILE_ACTION_RENAMED_OLD_NAME,
};

/// The fixed portion of a `FILE_NOTIFY_INFORMATION` record: three `u32` fields
/// (`NextEntryOffset`, `Action`, `FileNameLength`) ahead of the name.
const HEADER_LEN: usize = 12;

/// The size in bytes of one UTF-16 code unit in the wire format.
const UNIT_LEN: usize = 2;

/// The DWORD alignment of `FILE_NOTIFY_INFORMATION` records: every
/// `NextEntryOffset` is a multiple of this, and records begin on this boundary.
const RECORD_ALIGNMENT: usize = 4;

/// The `NextEntryOffset` value marking the final record of a chain. This is the
/// kernel's wire-format sentinel; changing it is a breaking change to the parser.
const FINAL_RECORD_OFFSET: usize = 0;

/// Byte offsets of the fixed `FILE_NOTIFY_INFORMATION` fields within a record.
///
/// These describe the kernel's wire layout; changing any value is a breaking
/// change to the parser.
mod field {
    /// `NextEntryOffset` (`u32`).
    pub const NEXT_ENTRY_OFFSET: usize = 0;
    /// `Action` (`u32`).
    pub const ACTION: usize = 4;
    /// `FileNameLength` (`u32`, in bytes).
    pub const FILE_NAME_LENGTH: usize = 8;
}

/// A file name reported by `ReadDirectoryChangesW`, relative to the watched
/// directory, preserved as raw UTF-16 so no information is lost.
///
/// The kernel delivers names as UTF-16 that need not be well-formed Unicode
/// (unpaired surrogates are possible on NTFS) and that are not NUL-terminated.
/// Storage is a [`Wtf16String`]: the same `u16` the kernel produced and the same
/// `u16` a wide (`*W`) Win32 API consumes, so a name can be reported and then
/// handed back to Windows without ever being re-encoded. Deref exposes the
/// borrowed [`Wtf16Str`] surface (`as_units`, `len`, `as_ptr`, `to_string_lossy`,
/// `has_interior_nul`).
///
/// [`RelativeName::as_wide`] exposes the raw units as the ground truth;
/// [`RelativeName::to_os_string`] and [`RelativeName::to_path_buf`] convert at the
/// boundary for callers wanting the platform types, losslessly in both directions
/// including for unpaired surrogates (D-8). (`OsStr::to_str` and `to_string_lossy`
/// remain lossy *views*, for display only.)
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct RelativeName {
    name: Wtf16String,
}

impl std::ops::Deref for RelativeName {
    type Target = Wtf16Str;

    fn deref(&self) -> &Wtf16Str {
        &self.name
    }
}

impl RelativeName {
    /// Build a name from raw UTF-16 units.
    ///
    /// Crate-internal: the kernel is the only source of real names in
    /// production, but the queue and monitor construct them in tests.
    pub(crate) fn from_units(units: Vec<u16>) -> Self {
        Self {
            name: Wtf16String::from_units(&units),
        }
    }

    /// The raw UTF-16 code units, exactly as the kernel reported them.
    #[must_use]
    pub fn as_wide(&self) -> &[u16] {
        self.name.as_units()
    }

    /// The name as native WTF-16, for handing straight to a wide Win32 API.
    ///
    /// The owned form is what carries the always-present terminator, so this is
    /// what an `LPCWSTR` parameter needs (`as_terminated_ptr`); the borrowed
    /// [`Deref`](std::ops::Deref) surface covers the counted `as_ptr` + `len`
    /// convention.
    #[must_use]
    pub fn as_wtf16(&self) -> &Wtf16String {
        &self.name
    }

    /// The name as an `OsString`, converting at the boundary losslessly.
    #[must_use]
    pub fn to_os_string(&self) -> OsString {
        self.name.to_os_string()
    }

    /// The name as a `PathBuf` (relative to the watched directory).
    #[must_use]
    pub fn to_path_buf(&self) -> PathBuf {
        PathBuf::from(self.to_os_string())
    }
}

impl std::fmt::Debug for RelativeName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Wtf16Str's Debug escapes a lone surrogate as `\u{d800}` rather than
        // collapsing it to U+FFFD, so a malformed name stays legible and
        // distinguishable in diagnostics.
        std::fmt::Debug::fmt(&self.name, f)
    }
}

/// One decoded record: the raw `FILE_ACTION_*` code and the relative name.
///
/// Internal to the crate: the public surface is the typed [`Change`], which loses
/// no fidelity ([`ChangeKind::Unknown`] preserves an unrecognised action code).
#[derive(Clone, Debug, PartialEq, Eq)]
struct RawChange {
    /// The raw `FILE_ACTION_*` value.
    action: u32,
    /// The reported name, relative to the watched directory.
    name: RelativeName,
}

/// Iterate the `FILE_NOTIFY_INFORMATION` chain in `buffer`.
///
/// `buffer` is the completion buffer truncated to the bytes the kernel returned.
/// Iteration stops at the first record that cannot be parsed cleanly -- a
/// truncated header, a name length that is odd or overruns the buffer, or a
/// `NextEntryOffset` that is unaligned, points into the current record, or falls
/// outside the buffer -- so a malformed or partial buffer can never cause an
/// out-of-bounds read. When iteration stops for such a reason the iterator sets
/// [`Records::malformed`]: the caller treats a malformed buffer as a desync
/// rather than a successful (possibly empty) batch, since a truncated buffer may
/// hide changes the client would otherwise believe it had seen. A clean end (a
/// final record with `NextEntryOffset == 0`) leaves `malformed` unset. A
/// zero-length buffer is the kernel's overflow signal and is handled by the
/// caller, not here.
///
/// Module-private: [`decode_batch`] is the crate-facing entrypoint, and it (the
/// sole non-test caller) inspects [`Records::malformed`] to turn a malformed
/// chain into a desync.
#[must_use]
fn records(buffer: &[u8]) -> Records<'_> {
    Records {
        buffer,
        pos: Some(0),
        malformed: false,
    }
}

/// Iterator over the record chain, returned by [`records`].
struct Records<'a> {
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
            // The preceding record's `NextEntryOffset` pointed past the buffer end.
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

        let next_offset = read_u32(rec, field::NEXT_ENTRY_OFFSET) as usize;
        let action = read_u32(rec, field::ACTION);
        let name_len_bytes = read_u32(rec, field::FILE_NAME_LENGTH) as usize;

        // This record spans from its header to the next record (`NextEntryOffset`),
        // or to the end of the buffer when it is the last record. The name must be
        // a whole number of UTF-16 code units (an even byte count) and lie entirely
        // within that span; a name that is odd, overruns the buffer, or reaches
        // past `NextEntryOffset` into the following record is malformed, and we
        // stop rather than read a partial code unit or another record's bytes.
        let record_span = if next_offset == FINAL_RECORD_OFFSET {
            rec.len()
        } else {
            next_offset.min(rec.len())
        };
        let name_end = match HEADER_LEN.checked_add(name_len_bytes) {
            Some(end) if end <= record_span && name_len_bytes.is_multiple_of(UNIT_LEN) => end,
            _ => {
                self.pos = None;
                self.malformed = true;
                return None;
            }
        };

        // A final record (`NextEntryOffset == 0`) must be the buffer's last bytes,
        // apart from the DWORD padding that aligns it. A name is a whole number of
        // UTF-16 units, so `name_end` is always even and its padding is exactly 0
        // or 2 bytes -- never 1 or 3. Any other remainder is data the chain does
        // not describe -- possibly further records whose link was zeroed, or a
        // truncated completion -- so treat it as malformed rather than silently
        // dropping the tail (which would understate the batch and lose changes).
        if next_offset == FINAL_RECORD_OFFSET {
            let padded_end = match name_end.checked_add(RECORD_ALIGNMENT - 1) {
                Some(sum) => sum & !(RECORD_ALIGNMENT - 1),
                None => {
                    self.pos = None;
                    self.malformed = true;
                    return None;
                }
            };
            if rec.len() != name_end && rec.len() != padded_end {
                self.pos = None;
                self.malformed = true;
                return None;
            }
        }

        let name = decode_utf16(&rec[HEADER_LEN..name_end]);

        // Advance to the next record, or finish. A well-formed `NextEntryOffset`
        // is a DWORD-aligned byte offset, measured from this record's start, that
        // clears this record's own header+name span. (An offset that reaches back
        // into the current record is already rejected above, by the
        // `name_end <= record_span` check, before this record is yielded.) An
        // offset that is unaligned or would overflow the position is a corrupt
        // link: the current record is still yielded, but the chain is marked
        // malformed and iteration stops.
        self.pos = if next_offset == FINAL_RECORD_OFFSET {
            None
        } else if next_offset.is_multiple_of(RECORD_ALIGNMENT) && next_offset >= name_end {
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

/// Copy a UTF-16LE byte region into owned code units. The caller rejects an odd
/// byte length as malformed before calling this, so `as_chunks` never has a
/// remainder to drop.
fn decode_utf16(bytes: &[u8]) -> RelativeName {
    let units: Vec<u16> = bytes
        .as_chunks::<UNIT_LEN>()
        .0
        .iter()
        .map(|pair| u16::from_le_bytes(*pair))
        .collect();
    RelativeName::from_units(units)
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
    /// The watch stopped *permanently* (D-22's non-retryable pair, discovered on a
    /// later re-establish attempt): nothing further will ever arrive for it. Unlike
    /// every other cause, a re-scan will not resynchronize this watch -- the client
    /// should treat it as ended and consult `Monitor::stop_reason` for why.
    Stopped,
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
