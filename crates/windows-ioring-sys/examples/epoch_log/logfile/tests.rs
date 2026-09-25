// Copyright (c) 2026 Mike Grier
//! Tests for the log file's shape (M25.3).
//!
//! # What these can and cannot establish
//!
//! They establish that the extent is really there and that the handle really
//! carries `FILE_FLAG_NO_BUFFERING`, both of which are checkable from here.
//!
//! They establish **nothing about whether a write pends**, and deliberately do
//! not try. Windows specifies nothing about when a ring operation completes
//! relative to `SubmitIoRing`, so a test asserting that an operation pends
//! would be asserting an observation about one machine as though it were a
//! contract -- and the log is required to be correct either way.
//!
//! They also cannot tell a written extent from a `set_len` one; see the module
//! docs for why, and the manifest's `notCoveredHere` for the record of it.

use std::io::Write;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::AsRawHandle;
use std::time::Duration;

use windows_ioring_sys::{Batch, IoRing, NumaBuffer, PushOptions, WriteCaching};

use crate::record::RECORD_STRIDE;

/// Hang bound on every wait here. Far above any real write.
const WAIT: Duration = Duration::from_secs(30);

/// A scratch path named per test, so tests running as threads in one process
/// cannot collide on it.
fn scratch(tag: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "windows-ioring-sys-epoch-logfile-{}-{tag}.tmp",
        std::process::id()
    ))
}

#[test]
fn the_extent_is_written_before_the_handle_is_returned() {
    const BLOCKS: usize = 4;
    let path = scratch("extent");
    let file = super::create_preallocated(&path, BLOCKS).expect("create the log file");

    let bytes = std::fs::read(&path).expect("read the extent back");
    assert_eq!(
        bytes.len(),
        BLOCKS * RECORD_STRIDE,
        "the file must already span every block a caller asked for, before a \
         single record is written -- an extending write is the configuration \
         the spike measured as behaving like a buffered one"
    );
    assert!(
        bytes.iter().all(|&byte| byte == 0),
        "a pre-allocated extent must read as zeros, which is also what lets \
         replay recognise the unwritten tail as NeverWritten"
    );

    drop(file);
    let _ = std::fs::remove_file(&path);
}

/// A synchronous write is refused, which is what `FILE_FLAG_OVERLAPPED` means.
///
/// This began as an attempt to test the `NO_BUFFERING` alignment rule through
/// [`std::io::Write`], and failed on the *aligned* write -- which was the
/// discovery, not the defect. `write_all` issues `WriteFile` with a null
/// `OVERLAPPED`, and an asynchronous handle refuses that however well-aligned
/// the transfer is. So this cannot test alignment, but it does test the other
/// flag, which nothing else here can reach: `GetFileInformationByHandleEx`
/// does not report the handle's mode, and the ring works on synchronous and
/// asynchronous handles alike.
///
/// The alignment rule is tested through the ring instead, below, which is also
/// how production reaches this handle.
#[test]
fn a_synchronous_write_is_refused_because_the_handle_is_overlapped() {
    let path = scratch("overlapped");
    let mut file = super::create_preallocated(&path, 2).expect("create the log file");

    // Sector-sized, sector-count-aligned, and inside the extent: everything
    // NO_BUFFERING asks of a transfer's *length*. What is left to object to is
    // the synchronous call itself.
    let aligned = vec![0xAB_u8; RECORD_STRIDE];
    let refused = file
        .write_all(&aligned)
        .expect_err("an overlapped handle must refuse a synchronous write");
    assert_eq!(
        refused.raw_os_error(),
        Some(windows_sys::Win32::Foundation::ERROR_INVALID_PARAMETER as i32),
        "the refusal must be the handle's mode and not some unrelated failure"
    );

    drop(file);
    let _ = std::fs::remove_file(&path);
}

/// The control for the test above: an ordinary handle accepts the same write.
///
/// Without it, the refusal could be caused by anything at all -- a bad path, a
/// closed handle, a length the filesystem disliked -- and the pair would agree
/// on a conclusion neither had established. This isolates the flags as the
/// only difference.
#[test]
fn an_ordinary_handle_accepts_the_write_an_overlapped_one_refuses() {
    let path = scratch("overlapped-control");
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .custom_flags(0)
        .open(&path)
        .expect("open an ordinary handle");

    file.write_all(&vec![0xAB_u8; RECORD_STRIDE])
        .expect("a synchronous handle has no objection to a synchronous write");

    drop(file);
    let _ = std::fs::remove_file(&path);
}

/// The handle really carries `NO_BUFFERING`, tested in **both** directions and
/// through the ring, which is how production reaches it.
///
/// A guard that only showed the aligned write succeeding would pass just as
/// happily against a buffered handle, which is exactly the regression worth
/// catching: dropping the flag changes nothing a caller can see except the
/// thing this whole item exists for. The refusal is what distinguishes them;
/// the acceptance is what shows the refusal is about alignment rather than the
/// handle being unusable.
///
/// The unaligned case breaks two of `NO_BUFFERING`'s three rules at once -- a
/// `Vec` guarantees no particular address, and its length is not a whole
/// number of sectors -- which is stated rather than tidied because it means
/// this test does not establish *which* rule refused it. It establishes that
/// the handle has rules a buffered one does not, which is the claim.
#[test]
fn the_ring_refuses_an_unaligned_write_and_accepts_an_aligned_one() {
    let path = scratch("alignment");
    let file = super::create_preallocated(&path, 2).expect("create the log file");
    let mut ring = IoRing::new(8, 8).expect("create ring");

    // Aligned on every axis: a `NumaBuffer` is page-granular and so
    // sector-granular (M22.3), the length is one whole stride, and the offset
    // is a block boundary.
    let buffer = NumaBuffer::new(RECORD_STRIDE, None).expect("allocate an aligned buffer");
    let mut batch = Batch::new(&mut ring);
    // SAFETY: `file` outlives the operation -- it is dropped at the end of this
    // test, after the completion is popped -- and the buffer is moved into the
    // token, which is held until then.
    let accepted = unsafe {
        batch.write_raw(
            file.as_raw_handle(),
            buffer,
            0,
            PushOptions::new(),
            WriteCaching::Cached,
        )
    }
    .expect("push the aligned write");
    batch.submit().expect("submit");
    let completion = ring
        .pop_within(WAIT)
        .expect("pop_within")
        .expect("the write completes well inside the bound");
    // Claimed before the result is inspected, which is the M22.2 discipline:
    // a token left unclaimed is still outstanding, whatever the write did.
    assert!(
        accepted.claim_if(&completion).is_ok(),
        "the completion must be the aligned write's"
    );
    completion
        .result()
        .expect("an aligned NO_BUFFERING write must be accepted");

    let mut batch = Batch::new(&mut ring);
    // SAFETY: as above.
    let rejected = unsafe {
        batch.write_raw(
            file.as_raw_handle(),
            vec![0xCD_u8; RECORD_STRIDE - 1],
            0,
            PushOptions::new(),
            WriteCaching::Cached,
        )
    }
    .expect("push the unaligned write");
    batch.submit().expect("submit");
    let completion = ring
        .pop_within(WAIT)
        .expect("pop_within")
        .expect("the write completes well inside the bound");
    assert!(
        rejected.claim_if(&completion).is_ok(),
        "the completion must be the unaligned write's"
    );
    completion
        .result()
        .expect_err("NO_BUFFERING must refuse a transfer that breaks its alignment rules");

    drop(file);
    let _ = std::fs::remove_file(&path);
}
