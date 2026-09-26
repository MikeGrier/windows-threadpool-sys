// Copyright (c) 2026 Mike Grier
//! The bounded pop against a genuinely pending operation (M21.6, M22+.1).
//!
//! # Why an integration test, and why a pipe
//!
//! No other test of [`IoRing::pop_within`] has a **real operation still pending
//! when a real bound expires**, and that is the exact state the Win32 timeout
//! path needs. The coverage either side of it misses in opposite directions:
//! the tests that use the ring's own `SubmitWait` drive operations that
//! *complete*, so the expiry branch is never reached, and the test that does
//! let a bound expire fakes both halves -- a `RecordingWait` instead of the
//! kernel, and a bare `reserve_user_data` instead of a pending operation.
//!
//! (An earlier version of this comment said every other test "drives the loop
//! with a wait that never enters the kernel". That is not so -- several reach
//! the kernel through `SubmitWait` -- and the imprecision mattered, because it
//! named the wrong gap. Corrected by `M24.6`'s sweep.)
//!
//! That gap hid a real defect: a
//! timed-out `SubmitIoRing` reports `ERROR_TIMEOUT`, so the crate's own
//! `SubmitWait` turned every expired bound into an `Err` instead of the
//! `Ok(None)` `pop_within` documents. Replacing `RingWait::block`'s whole body
//! with an unconditional error left the entire suite green.
//!
//! Closing that needs an operation that is **still pending** when the bound
//! expires. The first version of this file got one by reading 128 MiB
//! unbuffered, which is a *margin* rather than a guarantee -- and it failed once,
//! unreproducibly, for what was most likely that reason. What is used instead is
//! an operation that **cannot** complete until the test says so: a read on an
//! overlapped named pipe that nobody has written to.
//!
//! The difference is the whole point. A slow read asks "will the device take
//! longer than 5 ms?", which is a question about someone else's hardware. A
//! pipe with no writer asks nothing: the read is pending because no byte exists
//! to satisfy it, and it completes exactly when this file writes one.
//!
//! Three earlier attempts, each measured before being discarded:
//!
//! | Attempt | Result |
//! |---|---|
//! | Buffered read, up to 256 MiB | 3-5 us -- the cache answers |
//! | Flush over 512 MiB of dirty cache | 3 us -- the lazy writer got there first |
//! | Unbuffered read, 256 MiB, no `FILE_FLAG_OVERLAPPED` | 3 us -- a synchronous handle completes inline during submit |
//!
//! That third row is worth keeping: **a handle opened without
//! `FILE_FLAG_OVERLAPPED` is synchronous**, so a ring operation against it does
//! not pend at all. The pipe below is opened overlapped for exactly that reason.
//!
//! # What is and is not asserted about time
//!
//! Nothing here asserts that an operation finished within some bound. Where a
//! delay is needed -- `run_down` polls in 50 ms steps, so forcing it to see an
//! expired poll needs the release to come later than that -- it comes from a
//! `thread::sleep`, whose guarantee runs the safe way: a sleep may overshoot,
//! never undershoot. Windows' default timer resolution is about 15.6 ms, so a
//! 5 ms bound routinely takes 14-19 ms to expire; that is fine, because no
//! assertion depends on it.

#![cfg(windows)]

use std::io::Write;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::time::Duration;

use windows_ioring_sys::{Batch, CompletionWait, IoRing, PushOptions, RingWait, SubmitWait};

/// The ring these tests drive: it holds the read's buffer, so no test has to.
type PipeRing = IoRing<Vec<u8>>;
use windows_sys::Win32::Foundation::GENERIC_WRITE;
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_FLAG_OVERLAPPED, FILE_SHARE_READ, FILE_SHARE_WRITE,
    OPEN_EXISTING,
};
use windows_sys::Win32::System::Pipes::CreateNamedPipeW;

/// `PIPE_ACCESS_INBOUND`. The byte-type, byte-read-mode and wait bits are all
/// zero, so one named constant covers the mode argument.
const PIPE_ACCESS_INBOUND: u32 = 0x0000_0001;
const PIPE_MODE_BYTE_WAIT: u32 = 0;

/// The bound these tests give the pop. Any value would do: the read cannot
/// complete until the test writes, so this only decides how long the wait
/// blocks before reporting that it expired.
const SHORT_BOUND: Duration = Duration::from_millis(5);

/// Comfortably longer than `run_down`'s 50 ms poll, so a rundown is guaranteed
/// to see at least one expired poll before the read is released. A sleep may
/// overshoot and never undershoot, which is the direction that keeps this
/// deterministic.
const LONGER_THAN_A_RUNDOWN_POLL: Duration = Duration::from_millis(200);

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

/// An overlapped named pipe: a read on `server` stays pending until something
/// is written to `client`.
struct Pipe {
    server: OwnedHandle,
    client: Option<std::fs::File>,
}

impl Pipe {
    fn new(tag: &str) -> Self {
        let name = format!(
            r"\\.\pipe\windows-ioring-sys-bounded-pop-{tag}-{}",
            std::process::id()
        );
        let wide_name = wide(&name);

        // SAFETY: a valid NUL-terminated name and standard parameters; the
        // returned handle is owned here.
        let server = unsafe {
            CreateNamedPipeW(
                wide_name.as_ptr(),
                PIPE_ACCESS_INBOUND | FILE_FLAG_OVERLAPPED,
                PIPE_MODE_BYTE_WAIT,
                1,
                4096,
                4096,
                0,
                std::ptr::null(),
            )
        };
        assert!(!server.is_null(), "CreateNamedPipeW failed");
        // SAFETY: a fresh handle nothing else owns.
        let server = unsafe { OwnedHandle::from_raw_handle(server) };

        // SAFETY: the pipe exists; this opens its client end.
        let client = unsafe {
            CreateFileW(
                wide_name.as_ptr(),
                GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                std::ptr::null(),
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                std::ptr::null_mut(),
            )
        };
        assert!(
            !client.is_null() && client as isize != -1,
            "opening the pipe's client end failed"
        );
        // SAFETY: a fresh handle nothing else owns.
        let client = unsafe { OwnedHandle::from_raw_handle(client) };

        Self {
            server,
            client: Some(std::fs::File::from(client)),
        }
    }

    /// Queue a read that cannot complete, and submit it.
    fn push_pending_read(&self, ring: &mut PipeRing) {
        let mut batch = Batch::new(ring);
        // SAFETY: the server handle outlives the operation -- every test
        // releases and drains before dropping this `Pipe` -- and the buffer is
        // the ring's from here, returned by the pop that completes it.
        unsafe {
            batch.read_raw_owned(
                self.server.as_raw_handle(),
                vec![0_u8; 64],
                (),
                0,
                PushOptions::new(),
            )
        }
        .expect("queue a read on the pipe");
        batch.submit().expect("submit the read");
    }

    /// Satisfy the pending read, now.
    fn release(&mut self) {
        let mut client = self.client.take().expect("the client end is still open");
        client.write_all(b"x").expect("write to the pipe");
        client.flush().expect("flush the pipe");
    }

    /// Satisfy the pending read after `delay`, from another thread.
    ///
    /// Used where the test needs the operation to outlive something -- a
    /// `run_down` poll -- rather than merely to be pending.
    fn release_after(&mut self, delay: Duration) -> std::thread::JoinHandle<()> {
        let mut client = self.client.take().expect("the client end is still open");
        std::thread::spawn(move || {
            std::thread::sleep(delay);
            let _ = client.write_all(b"x");
            let _ = client.flush();
        })
    }
}

/// Release the read and drain it, so the ring is quiet before it drops.
fn settle(ring: &mut PipeRing, pipe: &mut Pipe) {
    pipe.release();
    // No token to hand in: the pop that observes the completion is what
    // returns the buffer, so there is no map to keep and nothing to match.
    let (completion, held) = loop {
        if let Some(popped) = ring.try_pop().expect("try_pop") {
            break popped;
        }
    };
    let bytes = completion.result().expect("the read succeeded");
    assert_eq!(bytes, 1, "exactly the byte that was written");
    let (buffer, ()) = held.expect("the ring held this read's buffer");
    let buffer = buffer.expect("a read carries a buffer");
    assert_eq!(buffer.len(), 64, "the buffer comes back as it went in");
}

#[test]
fn a_bound_that_expires_reports_no_completion_rather_than_an_error() {
    // The regression test for the defect this file exists for. Before the fix
    // this returned `Err(HRESULT 0x800705B4)`.
    let mut ring = PipeRing::with_inventory(16, 16).expect("create a ring");
    let mut pipe = Pipe::new("expires");
    pipe.push_pending_read(&mut ring);

    let popped = ring
        .pop_within(SHORT_BOUND)
        .expect("an expired bound is not an error");
    assert!(
        popped.is_none(),
        "nothing has been written to the pipe, so the read cannot have completed"
    );
    assert!(ring.outstanding() > 0, "and the read must still be pending");

    settle(&mut ring, &mut pipe);
}

/// Forwards to the ring's own wait and records what it was asked and what it
/// answered.
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
    let mut ring = PipeRing::with_inventory(16, 16).expect("create a ring");
    let mut pipe = Pipe::new("reached");
    pipe.push_pending_read(&mut ring);

    let mut wait = CountingSubmitWait::default();
    let popped = ring
        .pop_within_with(&mut wait, SHORT_BOUND)
        .expect("an expired bound is not an error");

    assert!(popped.is_none());
    assert!(
        wait.calls >= 1,
        "the ring's own wait must actually be entered"
    );
    assert!(
        wait.last_result_was_ok,
        "an expired `SubmitIoRing` wait must be reported as success, not as \
         ERROR_TIMEOUT -- this is the defect the whole file exists for"
    );

    settle(&mut ring, &mut pipe);
}

#[test]
fn the_default_wait_is_the_ring_wait() {
    // `pop_within` and `pop_within_with(&mut SubmitWait, ..)` must agree, so the
    // convenience cannot quietly diverge from the documented default.
    let mut ring = PipeRing::with_inventory(16, 16).expect("create a ring");
    let mut pipe = Pipe::new("default");
    pipe.push_pending_read(&mut ring);

    let popped = ring
        .pop_within_with(&mut SubmitWait, SHORT_BOUND)
        .expect("an expired bound is not an error");
    assert!(popped.is_none());
    assert!(ring.outstanding() > 0);

    settle(&mut ring, &mut pipe);
}

#[test]
fn run_down_tolerates_an_operation_slower_than_its_poll() {
    // `run_down` polls in 50 ms steps. Treating an expired poll as fatal made it
    // return `Err` with the operation still outstanding, so `Drop` asserted and
    // then closed the ring anyway -- the exact hazard it exists to prevent. The
    // release is deliberately later than one poll, so at least one poll is
    // guaranteed to expire before the read completes.
    let mut ring = PipeRing::with_inventory(16, 16).expect("create a ring");
    let mut pipe = Pipe::new("rundown");
    pipe.push_pending_read(&mut ring);
    let writer = pipe.release_after(LONGER_THAN_A_RUNDOWN_POLL);

    ring.run_down()
        .expect("a poll that expires is not a rundown failure");
    assert_eq!(
        ring.outstanding(),
        0,
        "rundown returns only once nothing is outstanding"
    );

    writer.join().expect("the writer thread");
    // The rundown reaped the completion, so the ring is holding nothing: its
    // inventory emptied as the completion was popped. Before this conversion
    // it was here that a caller dropped a token it could no longer claim.
    assert_eq!(ring.held(), 0, "rundown left nothing in the inventory");
}

#[test]
fn dropping_a_ring_with_a_pending_operation_does_not_panic() {
    // The same defect seen from where it actually bit: `Drop` runs the rundown,
    // and in a debug build a failed rundown is a `debug_assert`.
    let mut ring = PipeRing::with_inventory(16, 16).expect("create a ring");
    let mut pipe = Pipe::new("drop");
    pipe.push_pending_read(&mut ring);
    let writer = pipe.release_after(LONGER_THAN_A_RUNDOWN_POLL);

    drop(ring);

    writer.join().expect("the writer thread");
}

// ------------------------------------------------------------------------
// Relocated from `src/ring/tests.rs` at 9bc0350e (M24.3). Pure relocation;
// these reach nothing crate-private, which is what let them move.

#[test]
fn pop_within_returns_none_at_once_when_nothing_can_ever_arrive() {
    // With no operation outstanding no completion is possible, so the loop
    // answers before consulting any wait at all.
    //
    // **The `Ok(None)` is itself the proof, with no clock involved.** This goes
    // through the convenience, so the wait is `SubmitWait` -- and
    // `SubmitIoRing` answers `E_INVALIDARG` when asked to wait for a completion
    // the kernel has no pending operation for. Were the early return removed,
    // the loop would reach that wait and this would be an `Err`. Asserting
    // `Ok(None)` therefore distinguishes "returned early" from "waited", which
    // is exactly what an elapsed-time assertion was being asked to do -- and
    // unlike a clock, it cannot be wrong because the machine was busy.
    let mut ring = PipeRing::with_inventory(16, 16).expect("create ring");
    let popped = ring
        .pop_within(std::time::Duration::from_secs(30))
        .expect("the early return means the ring's own wait is never reached");
    assert!(popped.is_none(), "nothing was ever submitted");
}

#[test]
fn a_bound_the_clock_cannot_represent_does_not_panic() {
    // `Instant + Duration` panics on overflow, and `Duration::MAX` is a
    // reasonable spelling of "no deadline". Nothing is outstanding here, so
    // the early return answers before any deadline arithmetic matters.
    let mut ring = PipeRing::with_inventory(16, 16).expect("create ring");
    let popped = ring
        .pop_within(std::time::Duration::MAX)
        .expect("pop_within");
    assert!(popped.is_none());
}
