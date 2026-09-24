// Copyright (c) 2026 Mike Grier
//! Opening the log's file so a commit is separately observable (M25.3).
//!
//! # What this does and why the order matters
//!
//! Two steps, and neither is interchangeable with the other:
//!
//! 1. **Write the extent** with an ordinary handle, then drop it.
//! 2. **Reopen** the existing file with `FILE_FLAG_NO_BUFFERING |
//!    FILE_FLAG_OVERLAPPED`.
//!
//! [write-pending-spike.rs](../../design-sessions/spikes/write-pending-spike.rs)
//! measured four configurations and only this one behaved differently from a
//! plain buffered handle. `FILE_FLAG_OVERLAPPED` on its own changed nothing,
//! and `NO_BUFFERING` over a file the writes were *extending* behaved like a
//! buffered one -- the filesystem serialises writes past the valid-data length,
//! so an extending write does not get to be asynchronous however it was opened.
//!
//! # `set_len` is not a substitute for writing the zeros, and nothing here
//! catches the difference
//!
//! Step 1 writes a real buffer of zeros rather than calling
//! [`std::fs::File::set_len`]. Both produce a file of the right size whose
//! bytes read back as zero, so they are indistinguishable to everything in this
//! sample -- but only the write advances NTFS's **valid data length**, and the
//! valid data length is the thing that decides whether a later write is
//! extending. A `set_len` extent would leave every write in condition C, the
//! one that was measured as behaving like a buffered handle.
//!
//! This is stated rather than tested because it is not observable from here:
//! see the `notCoveredHere` note in [sabotage.json](../../sabotage.json). The
//! only user-mode way to read back a valid-data length is
//! `FSCTL_QUERY_FILE_REGIONS`, and putting that in a sample to check a property
//! the sample does not otherwise use would be machinery for its own sake.
//!
//! # What is deliberately *not* opened this way
//!
//! The **checkpoint** file stays buffered. `NO_BUFFERING` constrains the
//! buffer's alignment, the file offset, and the transfer length, and a
//! checkpoint record is a sixteen-byte `Vec` written at offset 0 -- it fails
//! all three. Striding the control plane to satisfy a flag it does not need
//! would be the tail wagging the dog; the control plane's correctness comes
//! from its covering flush (see [`crate::checkpoint`]), not from how its bytes
//! are cached.
//!
//! The **retired** file is likewise ordinary: it is written once with
//! [`std::fs::write`] and read by the reclaim worker, never through a ring.
//!
//! # This buys an opportunity, never a guarantee
//!
//! Windows specifies nothing about when a ring operation completes relative to
//! `SubmitIoRing`, so "the write pends" is an observation about a machine and
//! not a contract. The log is correct either way and nothing in it may depend
//! on an operation pending. What this shape changes is whether a commit is
//! *separately measurable*, which is M25.4's problem.

use std::fs::{File, OpenOptions};
use std::io;
use std::os::windows::fs::OpenOptionsExt;
use std::path::Path;

use windows_sys::Win32::Storage::FileSystem::{FILE_FLAG_NO_BUFFERING, FILE_FLAG_OVERLAPPED};

use crate::record::RECORD_STRIDE;

/// Create `path` with `blocks` zeroed record blocks already written, and return
/// a handle over that extent opened `NO_BUFFERING | OVERLAPPED`.
///
/// Sized in blocks rather than bytes because every writer in this sample lands
/// one record per [`RECORD_STRIDE`] block, so blocks are the unit a caller
/// actually knows -- and a byte count that was not a whole number of blocks
/// could not be written through the returned handle anyway.
///
/// # Errors
///
/// Any error from writing the extent or from reopening it.
pub fn create_preallocated(path: &Path, blocks: usize) -> io::Result<File> {
    let len = blocks
        .checked_mul(RECORD_STRIDE)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "log extent overflows"))?;

    // Written, not `set_len`-ed, and the distinction is the whole point of the
    // step -- see the module docs. The ordinary handle is dropped at the end of
    // this statement, before the reopen.
    std::fs::write(path, vec![0_u8; len])?;

    // No `create`, no `truncate`: this must be `OPEN_EXISTING`, because
    // truncating would discard the extent that is the only thing distinguishing
    // this from the configuration the spike measured as behaving like a
    // buffered handle.
    OpenOptions::new()
        .write(true)
        .custom_flags(FILE_FLAG_NO_BUFFERING | FILE_FLAG_OVERLAPPED)
        .open(path)
}

#[cfg(test)]
mod tests;
