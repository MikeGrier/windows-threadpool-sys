// Copyright (c) 2026 Mike Grier
//! Opening the log's file so a commit is separately observable (M25.3).
//!
//! # What this does and why the order matters
//!
//! Two steps, and neither is interchangeable with the other:
//!
//! 1. **Zero-fill the extent** with an ordinary handle, then drop it.
//! 2. **Reopen** the existing file with `FILE_FLAG_NO_BUFFERING |
//!    FILE_FLAG_OVERLAPPED`.
//!
//! [write-pending-spike.rs](../../design-sessions/spikes/write-pending-spike.rs)
//! measured five configurations, and what replicates across sixteen runs is
//! that a **buffered** handle essentially never pends while every
//! `NO_BUFFERING` one pends in most runs. Among the unbuffered conditions the
//! zero-filled extent has the highest rate and much the highest floor, which
//! is why the log uses it -- but the three overlap heavily and a single run of
//! any of them can land in another's range. See
//! [measurements/2026-09-24-set-len-vs-zero-fill/](../../measurements/2026-09-24-set-len-vs-zero-fill/README.md)
//! for the runs and for the earlier, stronger reading this replaced.
//!
//! # The zero-fill is not avoided, it is moved -- and moving it is not free
//!
//! Writing past NTFS's valid data length obliges the filesystem to zero-fill
//! the gap first. Step 1 does that zeroing once, eagerly, on an ordinary
//! handle, at a moment when nothing is being measured and no ring is involved.
//!
//! **For a sequential writer the total zeroing cost is the same either way**,
//! and that is measured: filling an extent after `set_len` costs what
//! zero-filling it outright costs (287 ms against 315 ms per GiB), because
//! every write lands exactly at the valid data length and none has a gap in
//! front of it. So this log is not buying cheaper zeroing.
//!
//! **What it is avoiding is the case where the bill arrives inside one write.**
//! A write that lands *past* the valid data length pays to zero the whole gap,
//! synchronously, before it proceeds: one sector written at the end of a
//! `set_len`'d 1 GiB file took roughly 2.3 seconds, about eight times the cost
//! of writing the entire extent. A log that only ever appends does not hit
//! that -- but a log is exactly the kind of program that later grows a
//! recovery path, a header rewrite, or a segment that seeks. See
//! [measurements/2026-09-24-set-len-zero-fill-cost/](../../measurements/2026-09-24-set-len-zero-fill-cost/README.md).
//!
//! # `set_len` is not a substitute for the zero-fill, and the difference is
//! measured rather than argued
//!
//! Step 1 writes a real buffer of zeros rather than calling
//! [`std::fs::File::set_len`]. Both produce a file of the right size whose
//! bytes read back as zero -- reads past the valid data length are answered
//! with zeros the filesystem synthesises without touching the disk -- so the
//! two are indistinguishable to everything in this sample. Only the write
//! advances the valid data length, which is the thing that decides whether a
//! later write is extending.
//!
//! An earlier version of this comment asserted that a `set_len` extent "would
//! leave every write in condition C". That was reasoned from documentation and
//! was challenged in review, so it was measured instead, twice over.
//!
//! **On zeroing cost, the assertion was simply the wrong mechanism.** For a
//! sequential writer `set_len` costs nothing extra -- see the section above.
//!
//! **On pending rate there is a real difference**, which is the measured reason
//! this function zero-fills. The spike gained a condition E over a `set_len`
//! extent, and sixteen runs are in
//! [measurements/2026-09-24-set-len-vs-zero-fill/](../../measurements/2026-09-24-set-len-vs-zero-fill/README.md):
//! the zero-filled extent pended at a median of 471/500 against `set_len`'s
//! 268/500, with a floor of 121 against 1. But the extending case and the
//! `set_len` case are not distinguishable from each other on that data, so the
//! claim that `set_len` *is* the extending case remains unsupported and is not
//! made.
//!
//! **Read those measurements before relying on any of this.** The first also
//! corrects a stronger claim this repository had been repeating -- that the
//! zero-filled extent was the only condition that pended at all. That descends
//! from a single run and does not replicate.
//!
//! (`SetFileValidData` moves the valid data length without writing anything,
//! which is how a database pre-allocates in one syscall. It is not used here:
//! it needs `SE_MANAGE_VOLUME_NAME`, and it exposes whatever bytes were
//! previously on those clusters to anything that reads the file. An earlier
//! draft of this paragraph said it saves "a few milliseconds of zeroing",
//! which was wrong by two to three orders of magnitude -- the measured cost is
//! ~300 ms per GiB done well, and seconds per GiB when forced onto a seeking
//! write. That cost is the whole reason the API exists.)
//!
//! Nothing in the test suite catches a swap to `set_len`: see the declared
//! blind spot in [sabotage.json](../../sabotage.json). The difference is a
//! *rate* that varies enormously run to run, so a test asserting it would be
//! asserting an observation about one machine as though it were a contract,
//! which `M25`'s standing constraint forbids.
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
/// Any error from the zero-fill or from reopening it.
pub fn create_preallocated(path: &Path, blocks: usize) -> io::Result<File> {
    let len = blocks
        .checked_mul(RECORD_STRIDE)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "log extent overflows"))?;

    // The zero-fill, done eagerly here rather than left for the filesystem to
    // do lazily on the ring's write path -- see the module docs, and note that
    // `set_len` is not a substitute however identical the resulting file looks.
    // The ordinary handle is dropped at the end of this statement, before the
    // reopen.
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
