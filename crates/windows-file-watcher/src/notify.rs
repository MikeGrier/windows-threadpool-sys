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

/// The fixed portion of a `FILE_NOTIFY_INFORMATION` record: three `u32` fields
/// (`NextEntryOffset`, `Action`, `FileNameLength`) ahead of the name.
const HEADER_LEN: usize = 12;

/// A file name reported by `ReadDirectoryChangesW`, relative to the watched
/// directory, preserved as raw UTF-16 so no information is lost.
///
/// The kernel delivers names as UTF-16 that need not be well-formed Unicode
/// (unpaired surrogates are possible on NTFS) and that are not NUL-terminated.
/// [`RelativeName::as_wide`] exposes the raw units; [`RelativeName::to_os_string`]
/// and [`RelativeName::to_path_buf`] round-trip them losslessly through the
/// platform's WTF-8 `OsString`.
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
/// The action is the raw `u32` the kernel wrote; mapping it to a typed change
/// kind is a separate concern.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawChange {
    /// The raw `FILE_ACTION_*` value.
    pub action: u32,
    /// The reported name, relative to the watched directory.
    pub name: RelativeName,
}

/// Iterate the `FILE_NOTIFY_INFORMATION` chain in `buffer`.
///
/// `buffer` is the completion buffer truncated to the bytes the kernel returned.
/// Iteration stops at the first record that would read out of bounds -- a
/// truncated header, a name length that overruns the buffer, or a
/// `NextEntryOffset` that points outside it -- so a malformed or partial buffer
/// can never cause an out-of-bounds read. A zero-length buffer yields nothing
/// (a zero-byte completion is the kernel's overflow signal, handled by the
/// caller, not here).
#[must_use]
pub fn records(buffer: &[u8]) -> Records<'_> {
    Records {
        buffer,
        pos: Some(0),
    }
}

/// Iterator over the record chain, returned by [`records`].
pub struct Records<'a> {
    buffer: &'a [u8],
    pos: Option<usize>,
}

impl Iterator for Records<'_> {
    type Item = RawChange;

    fn next(&mut self) -> Option<RawChange> {
        let pos = self.pos?;
        let rec = self.buffer.get(pos..)?;
        if rec.len() < HEADER_LEN {
            self.pos = None;
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
                return None;
            }
        };
        let name = decode_utf16(&rec[HEADER_LEN..name_end]);

        // Advance to the next record, or finish. A non-zero offset must make
        // forward progress without overflowing; the bounds check at the top of
        // the next call rejects an offset that points past the buffer.
        self.pos = if next_offset == 0 {
            None
        } else {
            match pos.checked_add(next_offset) {
                Some(next) if next > pos => Some(next),
                _ => None,
            }
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
