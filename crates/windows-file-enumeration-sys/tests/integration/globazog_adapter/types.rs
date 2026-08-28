// Copyright (c) 2026 Mike Grier
//! Types reconstructed from Globazog's Windows enumeration backend contract.
//!
//! # Provenance
//!
//! Globazog (`MikeGrier/globazog-rs`) is a separate repository, not a
//! dependency of this workspace, so these types cannot simply be imported.
//! They are reconstructed here, field for field, from the pinned commit
//! `55a0b1aec7a93051a675852636ab41a6437440fb`:
//!
//! - `FileId`, `DirEntry`, `EntryFailure`, `DirScan`, `EnumPlan` --
//!   `crates/globazog/src/sys.rs`
//! - `enumerate_dir_native`'s signature, the FILETIME-to-Unix-nanoseconds
//!   conversion, and the dot-entry filter -- `crates/globazog/src/sys/win.rs`
//! - `CodePoint` and `decode_utf16` -- `crates/globazog/src/syntax.rs` and
//!   `crates/globazog/src/syntax/decode.rs`
//!
//! If Globazog's real types ever drift from what is reconstructed here, this
//! file is what needs updating -- drift is not evidence that the replacement
//! this module demonstrates has become unsound.
//!
//! `EntryType::Other` exists in Globazog's vocabulary for platforms with
//! entries that are neither a file nor a directory (a device, a socket, a
//! FIFO). Windows' own attribute model has no such third kind, so it is kept
//! here for the predicate translation in
//! [`predicate_types`](super::predicate_types) to translate faithfully, even
//! though a Windows [`DirEntry`] never carries it.

use std::io;

/// A Unicode code point in Globazog's 32-bit matcher space. Unlike [`char`]
/// this may hold an unpaired surrogate, which is exactly the case this
/// adapter must not lose.
pub type CodePoint = u32;

/// The coarse kind of a directory entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryType {
    /// A regular file.
    File,
    /// A directory.
    Dir,
    /// Anything else. Never produced by this Windows adapter; see the module
    /// doc comment.
    Other,
}

/// A filesystem object identity for cycle detection: volume plus file id.
///
/// `{0, 0}` means unknown (identity not requested, or unobtainable), never a
/// volume-less id: a bare id without its volume is not globally meaningful.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FileId {
    /// The volume serial number.
    pub volume: u64,
    /// The 128-bit file id, within that volume.
    pub id: u128,
}

/// One enumerated directory entry with its inline metadata.
#[derive(Clone, Debug)]
pub struct DirEntry {
    /// The entry's own name, in code-point space.
    pub name: Vec<CodePoint>,
    /// The entry kind.
    pub entry_type: EntryType,
    /// Whether the entry is a reparse point.
    pub is_reparse: bool,
    /// The reparse tag (`0` when not a reparse point).
    pub reparse_tag: u32,
    /// The raw Win32 `FILE_ATTRIBUTE_*` bitmask.
    pub attributes: u32,
    /// File size in bytes -- Globazog's own `DirEntry` carries only this one
    /// size (`EndOfFile`, the logical size). See
    /// `tests_metadata::both_sizes_survive_in_the_replaced_engine` for proof
    /// that the *replaced* engine retains the allocation size too; Globazog's
    /// own struct simply never asked for it.
    pub size: u64,
    /// Birth / creation time, in nanoseconds since the Unix epoch.
    pub btime: i64,
    /// Last-modification time, in nanoseconds since the Unix epoch.
    pub mtime: i64,
    /// Last-access time, in nanoseconds since the Unix epoch.
    pub atime: i64,
    /// Metadata-change time, in nanoseconds since the Unix epoch.
    pub ctime: i64,
    /// Object identity for cycle detection.
    pub file_id: FileId,
}

/// The result of enumerating one directory: the entries read successfully,
/// plus any per-entry (in practice, only per-directory-read) failures.
#[derive(Debug)]
pub struct DirScan {
    /// The entries read successfully, with inline metadata.
    pub entries: Vec<DirEntry>,
    /// Late failures that truncated the listing without invalidating what was
    /// already read.
    pub entry_errors: Vec<EntryFailure>,
}

/// A per-entry or per-directory failure surfaced inside a [`DirScan`] rather
/// than aborting it.
#[derive(Debug)]
pub struct EntryFailure {
    /// The failing entry's name, when a specific entry can be attributed.
    /// `None` for a directory-level fault -- a late read that truncates the
    /// listing after some entries were already read -- which is the only
    /// case this Windows backend ever produces.
    pub name: Option<Vec<CodePoint>>,
    /// The underlying OS error.
    pub source: io::Error,
}

/// What the caller needs from each entry's metadata.
#[derive(Clone, Copy, Debug)]
pub struct EnumPlan {
    /// Fetch the stat-tier fields (size, timestamps, attributes).
    ///
    /// Moot for this backend, exactly as it is for Globazog's own real
    /// `win.rs`: every stat-tier field is inline in the same record the
    /// directory listing itself returns, so there is no separate per-entry
    /// query to skip -- exactly the "no per-entry opens" property this
    /// adapter demonstrates. The field is kept, unread, only because the
    /// real `EnumPlan` carries it too and a caller may still construct one.
    #[allow(
        dead_code,
        reason = "mirrors Globazog's own EnumPlan; its real win.rs backend never reads this field either, for the same reason"
    )]
    pub want_stat: bool,
    /// Fetch every entry's file identity (volume plus file id).
    pub want_file_id: bool,
    /// Fetch file identity for reparse points only.
    pub want_reparse_file_id: bool,
}

impl EnumPlan {
    /// Fetch everything.
    pub const FULL: EnumPlan = EnumPlan {
        want_stat: true,
        want_file_id: true,
        want_reparse_file_id: true,
    };

    /// Whether any file identity is requested.
    #[must_use]
    pub fn wants_any_file_id(&self) -> bool {
        self.want_file_id || self.want_reparse_file_id
    }

    /// Whether an entry with the given reparse status needs its file
    /// identity filled in.
    ///
    /// The replaced engine obtains every entry's identity at no extra
    /// per-entry cost once the volume is known -- it is part of the same
    /// batched record every other field comes from -- so there is nothing
    /// this adapter saves by skipping the computation itself. It still
    /// honors this exactly as Globazog's own backend does, withholding
    /// identity from an entry the plan did not ask it for, because the
    /// property under test is translation fidelity, not an optimization
    /// opportunity.
    #[must_use]
    pub fn wants_file_id_for(&self, is_reparse: bool) -> bool {
        self.want_file_id || (is_reparse && self.want_reparse_file_id)
    }
}

/// Decode a Windows UTF-16 name into code points, preserving unpaired
/// surrogates as their own code-point value. Never panics.
///
/// Ported verbatim from `crates/globazog/src/syntax/decode.rs`'s
/// `decode_utf16`; see the module doc comment for provenance.
#[must_use]
pub fn decode_utf16(units: &[u16]) -> Vec<CodePoint> {
    let mut out = Vec::with_capacity(units.len());
    let mut i = 0;
    while i < units.len() {
        let u = units[i];
        if (0xD800..=0xDBFF).contains(&u)
            && let Some(&lo) = units.get(i + 1)
            && (0xDC00..=0xDFFF).contains(&lo)
        {
            let cp = 0x1_0000 + (((u as u32) - 0xD800) << 10) + ((lo as u32) - 0xDC00);
            out.push(cp);
            i += 2;
            continue;
        }
        out.push(u as u32);
        i += 1;
    }
    out
}

/// The exact inverse of [`decode_utf16`]: encode code points back into
/// WTF-16 code units, re-forming a supplementary character as a surrogate
/// pair and re-emitting an already-unpaired surrogate value verbatim.
///
/// This is needed only by this adapter's predicate translator, which must
/// turn a Globazog name pattern's code points back into the WTF-16 units
/// `windows_file_enumeration_sys`'s pattern matcher operates on; Globazog
/// itself has no matching "encode" function because it never needs to go
/// this direction.
#[must_use]
pub fn encode_codepoint_to_wtf16(cp: CodePoint) -> Vec<u16> {
    if cp >= 0x1_0000 {
        let v = cp - 0x1_0000;
        let high = 0xD800 + (v >> 10);
        let low = 0xDC00 + (v & 0x3FF);
        vec![high as u16, low as u16]
    } else {
        vec![cp as u16]
    }
}

/// 100-ns intervals between the Windows (1601) and Unix (1970) epochs.
///
/// Ported from `crates/globazog/src/sys/win.rs`.
const FILETIME_TO_UNIX_100NS: i64 = 116_444_736_000_000_000;

/// Convert a raw Windows tick count (100-ns intervals since 1601-01-01) to
/// nanoseconds since the Unix epoch, exactly as Globazog's own
/// `filetime_to_unix_nanos` does: a `0` tick count -- a filesystem's sentinel
/// for "not tracked" -- stays `0` rather than becoming a real 1970 timestamp.
#[must_use]
pub fn filetime_to_unix_nanos(ticks: i64) -> i64 {
    if ticks == 0 {
        return 0;
    }
    (ticks - FILETIME_TO_UNIX_100NS).saturating_mul(100)
}

/// The inverse of [`filetime_to_unix_nanos`], needed only by this adapter's
/// predicate translator to turn a Globazog `Leaf::Time` value back into the
/// raw ticks `windows_file_enumeration_sys::WindowsFileTimestamp` compares
/// against. Lossy only in the sub-100-ns remainder, which
/// `filetime_to_unix_nanos`'s own forward conversion already discards.
#[must_use]
pub fn unix_nanos_to_filetime_ticks(nanos: i64) -> i64 {
    if nanos == 0 {
        return 0;
    }
    nanos / 100 + FILETIME_TO_UNIX_100NS
}
