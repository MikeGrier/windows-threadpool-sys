// Copyright (c) 2026 Mike Grier
//! The bounded pop against a genuinely pending operation (M21.6).
//!
//! # Why this is an integration test, and why it allocates so much
//!
//! Every other test of [`IoRing::pop_within`] drives the loop with a wait that
//! never enters the kernel, which makes the loop's arithmetic deterministic
//! and leaves the Win32 interaction untested. That gap hid a real defect: a
//! timed-out `SubmitIoRing` reports `ERROR_TIMEOUT`, so the crate's own
//! [`SubmitWait`] turned every expired bound into an `Err` instead of the
//! `Ok(None)` `pop_within` documents. Replacing `RingWait::block`'s whole body
//! with an unconditional error left the entire suite green.
//!
//! Closing that needs an operation that is **still pending** when the bound
//! expires, and on this platform that is harder to arrange than it sounds:
//!
//! - A buffered read completes from the cache. Measured at 3-5 us for sizes up
//!   to 256 MiB -- the completion is already poppable before the wait is
//!   reached.
//! - A flush over a large dirty cache is no better: the lazy writer has
//!   usually already written the data back, so the flush has nothing to do.
//!   Measured at 3 us for 512 MiB.
//! - **Unbuffered is not enough on its own.** A handle opened without
//!   `FILE_FLAG_OVERLAPPED` is a *synchronous* handle, and the read then
//!   completes inline during submit. Measured: 256 MiB unbuffered, 3 us.
//!
//! Only `FILE_FLAG_NO_BUFFERING | FILE_FLAG_OVERLAPPED` produces an operation
//! the ring genuinely has to wait for. That is why this file opens its fixture
//! the way it does, and why the buffer is page-aligned -- unbuffered I/O
//! requires it.
//!
//! **A bound is not precise, and these tests do not assume it is.** Windows'
//! default timer resolution is about 15.6 ms, so a 5 ms bound routinely takes
//! ~14-19 ms to expire. Nothing here asserts an upper bound on elapsed time;
//! what is asserted is *which answer* comes back, and that the operation was
//! still outstanding when it did.

#![cfg(windows)]

use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::AsRawHandle;
use std::path::PathBuf;
use std::time::Duration;

use windows_ioring_sys::{
    Batch, CompletionWait, IoBuf, IoBufMut, IoRing, PushOptions, RingWait, SubmitWait,
};

const FILE_FLAG_NO_BUFFERING: u32 = 0x2000_0000;
const FILE_FLAG_OVERLAPPED: u32 = 0x4000_0000;

/// Sector alignment for unbuffered I/O. 4096 covers every drive this is
/// expected to run on, and over-aligning is harmless.
const ALIGN: usize = 4096;

/// How large the pending read is.
///
/// Sized for margin rather than economy: at 16 MiB the read beat a 5 ms bound
/// on the development machine, at 64 MiB it did not. If this test ever fails
/// because the read completed too quickly, raise this rather than relaxing the
/// assertion -- the assertion is the whole point.
const READ_LEN: usize = 128 * 1024 * 1024;

/// The bound these tests give the pop. Far below how long the read takes, and
/// well under the timer granularity discussed above, so it always expires.
const SHORT_BOUND: Duration = Duration::from_millis(5);

/// A page-aligned owned buffer, which unbuffered I/O requires and `Vec<u8>`
/// does not guarantee.
struct Aligned {
    ptr: *mut u8,
    len: usize,
}

// SAFETY: `Aligned` owns its allocation exclusively and hands out no aliases,
// so moving it between threads moves the sole owner.
unsafe impl Send for Aligned {}

impl Aligned {
    fn new(len: usize) -> Self {
        let layout = std::alloc::Layout::from_size_align(len, ALIGN).expect("a valid layout");
        // SAFETY: `len` is non-zero and `ALIGN` is a valid power-of-two
        // alignment, so the layout is valid; zeroing initializes every byte,
        // which `IoBufMut` requires.
        let ptr = unsafe { std::alloc::alloc_zeroed(layout) };
        assert!(!ptr.is_null(), "allocating {len} bytes failed");
        Self { ptr, len }
    }
}

impl Drop for Aligned {
    fn drop(&mut self) {
        let layout = std::alloc::Layout::from_size_align(self.len, ALIGN).expect("a valid layout");
        // SAFETY: allocated by `new` with exactly this layout.
        unsafe { std::alloc::dealloc(self.ptr, layout) };
    }
}

// SAFETY: the allocation's address is fixed for the value's life -- moving
// `Aligned` moves only the pointer -- and `alloc_zeroed` initialized every
// byte.
unsafe impl IoBuf for Aligned {
    fn stable_ptr(&self) -> *const u8 {
        self.ptr
    }

    fn bytes_len(&self) -> usize {
        self.len
    }
}

// SAFETY: as `IoBuf`, and the allocation is uniquely owned, so a mutable
// pointer to it cannot alias anything.
unsafe impl IoBufMut for Aligned {
    fn stable_mut_ptr(&mut self) -> *mut u8 {
        self.ptr
    }
}

/// A fixture large enough that reading it unbuffered takes real time.
fn fixture(tag: &str) -> (PathBuf, std::fs::File) {
    let path = std::env::temp_dir().join(format!(
        "windows-ioring-sys-bounded-pop-{tag}-{}.bin",
        std::process::id()
    ));
    std::fs::write(&path, vec![0x5A_u8; READ_LEN]).expect("create the fixture");
    let file = std::fs::OpenOptions::new()
        .read(true)
        // Both flags are load-bearing, for different reasons -- see the module
        // docs. Without NO_BUFFERING the cache answers; without OVERLAPPED the
        // handle is synchronous and the read completes during submit.
        .custom_flags(FILE_FLAG_NO_BUFFERING | FILE_FLAG_OVERLAPPED)
        .open(&path)
        .expect("open the fixture unbuffered and overlapped");
    (path, file)
}

/// Queue the slow read and submit it, returning its token.
fn push_slow_read(ring: &mut IoRing, file: &std::fs::File) -> windows_ioring_sys::Token<Aligned> {
    let mut batch = Batch::new(ring);
    // SAFETY: `file` outlives the operation -- every test drains before
    // dropping it -- and the token is returned to the caller, which holds it
    // until the completion is claimed.
    let token = unsafe {
        batch.read_raw(
            file.as_raw_handle(),
            Aligned::new(READ_LEN),
            0,
            PushOptions::new(),
        )
    }
    .expect("queue the read");
    batch.submit().expect("submit the read");
    token
}

/// Drain the pending read so the ring is quiet before it drops.
fn settle(ring: &mut IoRing, token: windows_ioring_sys::Token<Aligned>) {
    let completion = ring
        .pop_within(Duration::from_secs(120))
        .expect("pop_within")
        .expect("the read completes eventually");
    let bytes = completion.result().expect("the read succeeded");
    assert_eq!(bytes, READ_LEN, "the whole fixture was read");
    let _ = token
        .claim_if(&completion)
        .expect("the token claims its own");
}

#[test]
fn a_bound_that_expires_reports_no_completion_rather_than_an_error() {
    // The regression test for the defect this file exists for. Before the
    // fix this returned `Err(HRESULT 0x800705B4)`.
    let mut ring = IoRing::new(16, 16).expect("create a ring");
    let (path, file) = fixture("expires");
    let token = push_slow_read(&mut ring, &file);

    let popped = ring
        .pop_within(SHORT_BOUND)
        .expect("an expired bound is not an error");
    assert!(
        popped.is_none(),
        "the read cannot have finished within {SHORT_BOUND:?}"
    );
    // Self-validating: if the read did finish, the assertion above is what
    // fails, and this says why the test was meaningful in the first place.
    assert!(
        ring.outstanding() > 0,
        "the read must still be pending, or this test proved nothing -- \
         raise READ_LEN rather than relaxing the assertion"
    );

    settle(&mut ring, token);
    drop(file);
    let _ = std::fs::remove_file(&path);
}

/// Forwards to the ring's own wait and counts how often it was reached.
#[derive(Default)]
struct CountingSubmitWait {
    calls: usize,
    last_result_was_ok: bool,
}

impl CompletionWait for CountingSubmitWait {
    fn wait(&mut self, ring: &mut RingWait<'_>, timeout_ms: u32) -> std::io::Result<()> {
        self.calls += 1;
        let outcome = ring.block(timeout_ms);
        self.last_result_was_ok = outcome.is_ok();
        outcome
    }
}

#[test]
fn the_rings_own_wait_is_reached_and_reports_an_expired_bound_as_success() {
    // Kills the mutation that started this: replacing `RingWait::block`'s body
    // with an unconditional error used to leave every test green, because no
    // test ever reached it.
    let mut ring = IoRing::new(16, 16).expect("create a ring");
    let (path, file) = fixture("reached");
    let token = push_slow_read(&mut ring, &file);

    let mut wait = CountingSubmitWait::default();
    let popped = ring
        .pop_within_with(&mut wait, SHORT_BOUND)
        .expect("an expired bound is not an error");

    assert!(popped.is_none(), "the read cannot have finished that fast");
    assert!(
        wait.calls >= 1,
        "the ring's own wait must actually be entered"
    );
    assert!(
        wait.last_result_was_ok,
        "an expired `SubmitIoRing` wait must be reported as success, not as \
         ERROR_TIMEOUT -- this is the defect the whole file exists for"
    );

    settle(&mut ring, token);
    drop(file);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn the_default_wait_is_the_ring_wait() {
    // `pop_within` and `pop_within_with(&mut SubmitWait, ..)` must agree, so
    // the convenience cannot quietly diverge from the documented default.
    let mut ring = IoRing::new(16, 16).expect("create a ring");
    let (path, file) = fixture("default");
    let token = push_slow_read(&mut ring, &file);

    let popped = ring
        .pop_within_with(&mut SubmitWait, SHORT_BOUND)
        .expect("an expired bound is not an error");
    assert!(popped.is_none());
    assert!(ring.outstanding() > 0);

    settle(&mut ring, token);
    drop(file);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn run_down_tolerates_an_operation_slower_than_its_poll() {
    // `run_down` polls in 50 ms steps. Treating an expired poll as fatal made
    // it return `Err` with the operation still outstanding, so `Drop`
    // asserted and then closed the ring anyway -- the exact hazard it exists
    // to prevent. Not reached by the rest of the suite only because every
    // other test's operations finish in well under 50 ms.
    let mut ring = IoRing::new(16, 16).expect("create a ring");
    let (path, file) = fixture("rundown");
    let token = push_slow_read(&mut ring, &file);

    ring.run_down()
        .expect("a poll that expires is not a rundown failure");
    assert_eq!(
        ring.outstanding(),
        0,
        "rundown returns only once nothing is outstanding"
    );

    // The completion was reaped by the rundown, so the token has nothing left
    // to claim; dropping it here is the honest end of its life.
    drop(token);
    drop(file);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn dropping_a_ring_with_a_slow_operation_does_not_panic() {
    // The same defect seen from where it actually bit: `Drop` runs the
    // rundown, and in a debug build a failed rundown is a `debug_assert`.
    let mut ring = IoRing::new(16, 16).expect("create a ring");
    let (path, file) = fixture("drop");
    let token = push_slow_read(&mut ring, &file);

    drop(token);
    drop(ring);

    drop(file);
    let _ = std::fs::remove_file(&path);
}
