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
//! # The forcing can be used on purpose, and is a real alternative
//!
//! Raised in review: since a write past the valid data length forces the fill
//! anyway, it can be *triggered* deliberately -- `set_len` to the final size,
//! then write one sector at the very end, and the filesystem zero-fills
//! everything in front of it. That was measured as condition F and it reaches
//! the same end state this function does; over sixteen runs it had the highest
//! floor of any condition (147/500 against the zero-fill's 65) at a
//! comparable median.
//!
//! It is a genuine trade rather than a strictly worse option:
//!
//! - **It needs no buffer at all** -- two syscalls, whatever the extent's size.
//! - **It costs about eight times the wall time** for a large extent, because
//!   the filesystem's own fill is much slower than a sequential write of the
//!   same bytes.
//!
//! This function takes the explicit fill because the cost is bounded and
//! predictable and the memory is now bounded too. A caller pre-allocating tens
//! of gigabytes, who would rather spend wall time than write the loop, has the
//! other option and it works.
//!
//! **None of this is perceptible at this sample's own sizes**, and that is
//! measured too: the log's extent is 140 KiB and each strategy file is 8 MiB,
//! so a whole run zero-fills 24 MiB in tens of milliseconds against a run that
//! takes over a second. At 140 KiB the cost is dominated by creating the file
//! rather than by writing zeros into it. The choice here is made for what this
//! code *teaches* a log that pre-allocates in gigabytes, not for what it costs
//! the sample.
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
use std::io::Write;
use std::os::windows::fs::OpenOptionsExt;
use std::path::Path;

use windows_sys::Win32::Storage::FileSystem::{FILE_FLAG_NO_BUFFERING, FILE_FLAG_OVERLAPPED};

use crate::record::RECORD_STRIDE;

/// Bytes per write while zero-filling the extent.
///
/// The fill used to be a single `std::fs::write` of a `vec![0; len]`, which
/// allocates the **whole extent** in memory -- harmless for this sample's
/// handful of blocks and a bad pattern for a log to copy, since a real one
/// pre-allocates in gigabytes. A fixed chunk keeps the fill's memory cost
/// constant in the size of the extent.
///
/// # Why 64 KiB, measured rather than picked
///
/// This was 1 MiB first, which is the worst of the plausible values on two
/// counts, both in
/// [measurements/2026-09-24-allocation-knee/](../../measurements/2026-09-24-allocation-knee/README.md):
///
/// - **Throughput stops improving at about 64 KiB.** Filling at 4 KiB chunks
///   runs at a few hundred MiB/s; by 64 KiB it is within run-to-run variance
///   of every larger size tried, up to 4 MiB. So a bigger chunk buys nothing.
/// - **1 MiB is exactly where this allocator stops using the heap.** Measured
///   with `VirtualQuery`: at 512 KiB and below, sixty-four live allocations
///   share a handful of reservations; at 1 MiB every one gets its own. So the
///   first size with no throughput benefit is also the first size that
///   guarantees a reservation and its teardown on every call.
///
/// 64 KiB is additionally the Windows virtual-memory allocation granularity,
/// which is why it is a good habit as well as a good measurement: it is the
/// point past which a block cannot share its region with anything else.
///
/// # Why this is allocated rather than a `static` array of zeros
///
/// A `static ZEROS: [u8; 64 * 1024]` would remove the allocation entirely --
/// no heap, no knee to reason about, pages arriving demand-zero from the
/// loader. On every axis a performance reader would check it is the better
/// choice, which is exactly why the reason to refuse it is written down here:
/// **it is a security decision and no measurement will surface it.**
///
/// A `static` lives at a fixed offset within the module, so a process that
/// leaks any module base thereby knows the address of a large, writable,
/// zero-filled region -- a ready-made landing pad for staging data, at an
/// address ASLR no longer protects once the base is known, present for the
/// life of the process whether or not a log is ever opened. A transient heap
/// allocation has neither property: its address is unpredictable and it exists
/// only while the fill is running.
///
/// The cost of declining the `static` is one allocation per call to
/// [`create_preallocated`], which the measurements above put at nothing worth
/// having.
const FILL_CHUNK: usize = 64 * 1024;

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
    // `set_len` alone is not a substitute however identical the resulting file
    // looks. The ordinary handle is dropped at the end of this block, before
    // the reopen.
    {
        let mut file = File::create(path)?;
        let chunk = vec![0_u8; FILL_CHUNK.min(len.max(1))];
        let mut written = 0;
        while written < len {
            let take = chunk.len().min(len - written);
            file.write_all(&chunk[..take])?;
            written += take;
        }
        file.flush()?;
    }

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
