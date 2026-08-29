// Copyright (c) 2026 Mike Grier
//! M12.2: the flush-barrier contract, tested against the *measured* behaviour
//! rather than against the implementation.
//!
//! [`FlushCoverage`] exists because an unflagged flush does not cover the
//! writes queued before it (D-23). A unit test can pin the enum-to-SQE-flag
//! mapping, and `batch/tests.rs` does -- but that only proves the flag is set,
//! not that setting it changes what the kernel does. This proves the
//! behaviour: with [`FlushCoverage::Unordered`] the flush is observed
//! completing while writes pushed before it are still outstanding, and with
//! [`FlushCoverage::CoversPrecedingOperations`] it never is.
//!
//! # Why the unusual conditions
//!
//! The spike behind D-23 needed three iterations to become valid, and each
//! failure mode is reproduced here as a requirement rather than rediscovered:
//!
//! 1. **Buffered writes could not discriminate.** They land in the page cache
//!    and finish in issue order, so every completion arrived in submission
//!    order even with no ordering flag. Hence `FILE_FLAG_NO_BUFFERING`, which
//!    is a measurement instrument here and not a recommendation.
//! 2. **Extending writes could not discriminate**, because the filesystem
//!    serializes writes past the valid-data length. Hence the pre-written
//!    extent.
//! 3. **Uniform write sizes could not discriminate.** What actually exposes
//!    reordering is a *size asymmetry*: the baseline measured 28 of 32 small
//!    writes overtaking large ones with no flags at all. Hence the two phases
//!    below -- 32 large writes before the flush, 32 tiny ones after it.
//!
//! A control that shows the same result as the treatment measures nothing, so
//! the [`FlushCoverage::Unordered`] case runs *first, as the control*. If it
//! shows no reordering on this machine, the covering assertion would pass for
//! the wrong reason, and the test skips loudly instead of passing quietly.

#![cfg(windows)]

use std::collections::HashMap;
use std::ffi::c_void;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle};
use std::path::{Path, PathBuf};

use windows_ioring_sys::{Batch, FlushCoverage, IoBuf, IoRing, PushOptions, Token};
use windows_sys::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_FLAG_NO_BUFFERING, FILE_FLAG_OVERLAPPED,
    FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};

/// Writes per phase. Enough that a reordering, if the device does any, is
/// visible rather than a coin flip; the spike measured 17 and 23 of 32.
const PHASE_OPS: usize = 32;

/// Phase A: large, so these writes are genuinely slow and can be overtaken.
const BIG_LEN: usize = 1024 * 1024;

/// Phase B: tiny, so these writes are genuinely fast and *could* overtake.
const SMALL_LEN: usize = 4096;

/// `FILE_FLAG_NO_BUFFERING` requires the buffer address, the file offset, and
/// the length to be sector-aligned. 4096 satisfies both 512e and 4Kn devices.
const ALIGN: usize = 4096;

/// Phase B writes here, past everything phase A touches, so the two phases
/// never contend for the same blocks.
const PHASE_B_BASE: usize = PHASE_OPS * BIG_LEN;

/// Generous: this is tens of MiB of unbuffered device I/O per case.
const WAIT_MS: u32 = 120_000;

/// A heap buffer whose first byte is `ALIGN`-aligned.
///
/// `Vec<u8>` gives no alignment guarantee beyond `u8`'s, so this over-allocates
/// and offsets into the allocation. The heap block itself does not move when
/// the `Aligned` value is moved, which is what [`IoBuf`] requires.
struct Aligned {
    storage: Vec<u8>,
    offset: usize,
    len: usize,
}

impl Aligned {
    fn new(len: usize, fill: u8) -> Self {
        let storage = vec![fill; len + ALIGN];
        let base = storage.as_ptr() as usize;
        let offset = (ALIGN - (base % ALIGN)) % ALIGN;
        Self {
            storage,
            offset,
            len,
        }
    }
}

// SAFETY: `storage` is a heap allocation that is never reallocated or resized
// while this value exists, so the address it reports is stable across moves and
// for the value's whole life; `offset` is within the over-allocation by
// construction, and `len` bytes from there are initialized and stay allocated.
// `len` is fixed at construction.
unsafe impl IoBuf for Aligned {
    fn stable_ptr(&self) -> *const u8 {
        // SAFETY: `offset <= ALIGN` and the allocation is `len + ALIGN` bytes.
        unsafe { self.storage.as_ptr().add(self.offset) }
    }

    fn bytes_len(&self) -> usize {
        self.len
    }
}

fn temp_file(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "windows-ioring-sys-flush-barrier-{tag}-{}.tmp",
        std::process::id()
    ))
}

/// Open `path` for unbuffered, overlapped access.
fn open_unbuffered(path: &Path) -> OwnedHandle {
    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: `wide` is a null-terminated wide path that outlives the call; the
    // security-attributes and template arguments are the documented null
    // defaults.
    let raw = unsafe {
        CreateFileW(
            wide.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OVERLAPPED | FILE_FLAG_NO_BUFFERING,
            std::ptr::null_mut(),
        )
    };
    assert!(
        !raw.is_null() && raw != INVALID_HANDLE_VALUE,
        "CreateFileW failed: {}",
        std::io::Error::last_os_error()
    );
    // SAFETY: `CreateFileW` just returned a fresh handle nothing else owns.
    unsafe { OwnedHandle::from_raw_handle(raw.cast::<c_void>()) }
}

/// What one case observed about completion order.
struct Observed {
    /// Phase-A writes (queued *before* the flush) that completed *after* it.
    /// This is D-23's direct observable; the spike saw 17 and 23 of 32.
    a_after_flush: usize,
    /// Phase-B writes (queued *after* the flush) that completed *before* it.
    /// D-24's observable: the barrier holds these back.
    b_before_flush: usize,
}

impl Observed {
    /// Did this case see the completion queue depart from submission order at
    /// all? Either observable will do -- see the control's own comment for why
    /// both are needed.
    fn saw_reordering(&self) -> bool {
        self.a_after_flush > 0 || self.b_before_flush > 0
    }
}

/// Push phase A, then one flush with `coverage`, then phase B; submit as one
/// batch and report the order in which the completions arrived.
fn run_case(ring: &mut IoRing, file: RawHandle, coverage: FlushCoverage) -> Observed {
    let mut pending: HashMap<usize, Token<Aligned>> = HashMap::new();
    let mut expected_len: HashMap<usize, usize> = HashMap::new();
    let mut phase_a = Vec::with_capacity(PHASE_OPS);
    let mut phase_b = Vec::with_capacity(PHASE_OPS);
    let flush_id;
    {
        let mut batch = Batch::new(ring);
        for index in 0..PHASE_OPS {
            let buffer = Aligned::new(BIG_LEN, index as u8);
            let offset = (index * BIG_LEN) as u64;
            // SAFETY: `file` stays open for the whole test, and every token is
            // held in `pending` until its completion has been popped.
            let token = unsafe { batch.write_raw(file, buffer, offset, PushOptions::new()) }
                .expect("queue phase-A write");
            phase_a.push(token.id());
            expected_len.insert(token.id(), BIG_LEN);
            pending.insert(token.id(), token);
        }

        // SAFETY: as above.
        flush_id = unsafe { batch.flush_raw(file, coverage) }.expect("queue flush");

        for index in 0..PHASE_OPS {
            let buffer = Aligned::new(SMALL_LEN, index as u8);
            let offset = (PHASE_B_BASE + index * SMALL_LEN) as u64;
            // SAFETY: as above.
            let token = unsafe { batch.write_raw(file, buffer, offset, PushOptions::new()) }
                .expect("queue phase-B write");
            phase_b.push(token.id());
            expected_len.insert(token.id(), SMALL_LEN);
            pending.insert(token.id(), token);
        }

        batch
            .submit_and_wait((PHASE_OPS * 2 + 1) as u32, WAIT_MS)
            .expect("submit and wait for every completion");
    }

    // The completion queue is FIFO, so pop order *is* completion order.
    let expected = PHASE_OPS * 2 + 1;
    let mut order = Vec::with_capacity(expected);
    let mut attempts = 0;
    while order.len() < expected {
        attempts += 1;
        assert!(
            attempts <= expected * 64,
            "only {} of {expected} completions ever arrived",
            order.len()
        );
        while let Some(completion) = ring.try_pop().expect("pop completion") {
            let transferred = completion.result().expect("write or flush succeeded");
            // A short write would make every count below meaningless, and an
            // unbuffered write with a misaligned length or offset is exactly
            // how that happens. Check rather than assume the I/O was real.
            if let Some(&len) = expected_len.get(&completion.user_data()) {
                assert_eq!(
                    transferred, len,
                    "an unbuffered write transferred {transferred} of {len} bytes"
                );
            }
            if let Some(token) = pending.remove(&completion.user_data()) {
                let _buffer = token
                    .claim_if(&completion)
                    .expect("a token claims its own completion");
            }
            order.push(completion.user_data());
        }
    }

    let flush_position = order
        .iter()
        .position(|&id| id == flush_id)
        .expect("the flush's own completion");
    Observed {
        a_after_flush: order[flush_position + 1..]
            .iter()
            .filter(|id| phase_a.contains(id))
            .count(),
        b_before_flush: order[..flush_position]
            .iter()
            .filter(|id| phase_b.contains(id))
            .count(),
    }
}

#[test]
fn a_covering_flush_waits_for_preceding_writes_and_an_unordered_one_does_not() {
    let extent = PHASE_B_BASE + PHASE_OPS * SMALL_LEN;
    let path = temp_file("cases");
    // Pre-written so that nothing below extends the file (failure mode 2).
    std::fs::write(&path, vec![0_u8; extent]).expect("pre-write the extent");
    let file = open_unbuffered(&path);
    let handle = file.as_raw_handle();

    // Both cases share one ring and one file so they are directly comparable;
    // each drains fully before the next begins. The submission queue must hold
    // a whole case at once.
    let mut ring = IoRing::new(256, 256).expect("create ring");

    // The control, and it runs first on purpose.
    //
    // Two observables are accepted, because which one moves is a property of
    // the device stack rather than of the contract. On the spike's machine the
    // flush overtook phase A (17 and 23 of 32). On the machine this test was
    // written on, phase A never completes after the flush even unflagged --
    // the stack appears to order a flush behind that file's outstanding writes
    // by itself -- but 11 of 32 phase-B writes overtake it, so the queue is
    // demonstrably not in submission order and a barrier is still
    // distinguishable. Requiring D-23's specific observable would have made
    // this test skip on hardware where it has plenty to say.
    let unordered = run_case(&mut ring, handle, FlushCoverage::Unordered);

    if !unordered.saw_reordering() {
        drop(ring);
        drop(file);
        let _ = std::fs::remove_file(&path);
        eprintln!(
            "SKIP: an unordered flush produced completions in strict submission order on this \
             machine, so there is no reordering here to distinguish a barrier from, and the \
             covering assertions would pass for the wrong reason."
        );
        return;
    }

    let covering = run_case(&mut ring, handle, FlushCoverage::CoversPrecedingOperations);

    drop(ring);
    drop(file);
    let _ = std::fs::remove_file(&path);

    // The contract (D-23). Not "usually waits": if the flush could complete
    // before even one preceding write, its completion would not prove those
    // writes are durable, which is the entire reason the variant exists.
    assert_eq!(
        covering.a_after_flush, 0,
        "a covering flush must not complete before any write queued ahead of it, but {} of \
         {PHASE_OPS} did (the unordered control saw {})",
        covering.a_after_flush, unordered.a_after_flush
    );

    // The barrier's other half (D-24): operations pushed after a drained op
    // are held until it completes.
    assert_eq!(
        covering.b_before_flush, 0,
        "a covering flush must also hold back the writes queued after it, but {} of {PHASE_OPS} \
         completed first (the unordered control saw {})",
        covering.b_before_flush, unordered.b_before_flush
    );
}
