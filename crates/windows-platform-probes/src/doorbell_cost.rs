// Copyright (c) Mike Grier.

//! How expensive is a doorbell, relative to the syscall it would guard?
//!
//! **An experiment, not a component.** These probes measure platform behaviour
//! and are not for production use. Do not call them from production code, and
//! do not lift a technique out of here. See this crate's DESIGN-NOTES.md.
//!
//! # The decision this exists to inform
//!
//! The two-layer ring design has a client thread push a descriptor onto a
//! bounded MPSC queue and then, sometimes, signal an event so the domain thread
//! wakes. The design assumes that signal is expensive enough to be worth
//! avoiding, and proposes an eventcount -- publish intent to park, re-check the
//! queue, then wait -- so a producer rings the doorbell only on the
//! empty-to-non-empty edge and only when a consumer is actually parked.
//!
//! That protocol is the highest-risk part of the whole design, because
//! publish-recheck-park is exactly where lost wakeups live. Building it because
//! the cost was *assumed* would be taking on that risk without evidence. So:
//!
//!   - if `SetEvent` is a meaningful fraction of `SubmitIoRing`, the skip rules
//!     are load-bearing and belong in the design from the start;
//!   - if it is noise, a simple always-signal queue is adequate and the
//!     optimization can wait for a measurement that justifies it.
//!
//! # What is timed
//!
//! Each is a tight loop over a warm path, reported as nanoseconds per
//! operation. Absolute values are host-specific and uninteresting; the
//! **ratios** are the finding.
//!
//! - `atomic_fetch_add` -- the uncontended atomic that a queue push costs, as a
//!   floor for "the cheapest useful thing".
//! - `set_event_already_signalled` -- `SetEvent` on an event that is already
//!   set, which is the redundant-signal case the skip rule removes.
//! - `set_reset_event` -- `SetEvent` then `ResetEvent`, the honest cost of one
//!   doorbell cycle with nobody waiting.
//! - `wait_zero_signalled` -- `WaitForSingleObject(handle, 0)` on a signalled
//!   event: the consumer's cost of observing it.
//! - `submit_io_ring_empty` -- `SubmitIoRing` with nothing queued, which is the
//!   syscall the doorbell would be amortised against. Absent when `IoRing` is
//!   unavailable.
//!
//! # The empty submit is not a fair denominator, and the first run proved it
//!
//! This probe was written expecting to divide the doorbell cost by
//! `submit_io_ring_empty` and read off "the doorbell is N% of a syscall". **Do
//! not do that.** Measured on the development machine, an empty `SubmitIoRing`
//! came in at ~79 ns -- far too cheap for a kernel transition, so it is almost
//! certainly short-circuiting in user mode when there is nothing queued. The
//! resulting "doorbell is 210% of a syscall" would have been a confident wrong
//! answer built on a denominator that never entered the kernel.
//!
//! The honest denominator is the cost of the real work a submission carries,
//! which this probe deliberately does not measure -- so it reports the absolute
//! costs and the *batching* arithmetic instead, and leaves the ratio alone.
//! [`Observation::doorbell_share_of_submit`] is retained only because the raw
//! fact is worth recording; its own documentation repeats this warning.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows_sys::Win32::System::Threading::{
    CreateEventW, ResetEvent, SetEvent, WaitForSingleObject,
};

use crate::ioring;

/// Nanoseconds per operation for one timed loop.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Timing {
    /// What was timed.
    pub label: &'static str,
    /// Iterations executed.
    pub iterations: u32,
    /// Nanoseconds per iteration.
    pub nanos_per_op: f64,
}

/// Every timing, plus the ratios that actually decide the design question.
#[derive(Debug, Clone)]
pub struct Observation {
    /// Each timed loop, in the order run.
    pub timings: Vec<Timing>,
    /// `None` when `IoRing` is unavailable on this host.
    pub submit_nanos: Option<f64>,
}

impl Observation {
    /// Look a timing up by label.
    #[must_use]
    pub fn get(&self, label: &str) -> Option<f64> {
        self.timings
            .iter()
            .find(|t| t.label == label)
            .map(|t| t.nanos_per_op)
    }

    /// One doorbell cycle as a fraction of one **empty** `SubmitIoRing`.
    ///
    /// **This is not the number the design turns on, and it should not be read
    /// as one.** An empty submit does not appear to enter the kernel (see the
    /// module documentation), so this ratio has a denominator that is not a
    /// syscall. It is exposed because the raw fact is worth recording across
    /// hosts -- a machine where the empty submit is *expensive* would itself be
    /// a finding -- not because dividing by it answers anything.
    #[must_use]
    pub fn doorbell_share_of_submit(&self) -> Option<f64> {
        let doorbell = self.get("set_reset_event")?;
        let submit = self.submit_nanos?;
        (submit > 0.0).then_some(doorbell / submit)
    }
}

fn time_loop(label: &'static str, iterations: u32, mut body: impl FnMut()) -> Timing {
    // Warm the path first: the first call through a syscall stub pays for
    // resolution and page faults that a steady-state cost should not include.
    for _ in 0..1024 {
        body();
    }
    let start = Instant::now();
    for _ in 0..iterations {
        body();
    }
    let elapsed = start.elapsed();
    Timing {
        label,
        iterations,
        nanos_per_op: elapsed.as_nanos() as f64 / f64::from(iterations),
    }
}

/// Run every timing.
///
/// # Panics
///
/// Panics if `CreateEventW` fails, which would mean the host cannot create a
/// manual-reset event and nothing here is measurable.
#[must_use]
pub fn measure() -> Observation {
    const ITERATIONS: u32 = 200_000;

    // SAFETY: a manual-reset, initially-unsignalled, unnamed event.
    let event: HANDLE = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
    assert!(!event.is_null(), "CreateEventW failed");

    let counter = AtomicU64::new(0);
    let mut timings = Vec::new();

    timings.push(time_loop("atomic_fetch_add", ITERATIONS, || {
        counter.fetch_add(1, Ordering::Relaxed);
    }));

    // Leave it signalled, so every call in the next loop is redundant.
    unsafe { SetEvent(event) };
    timings.push(time_loop("set_event_already_signalled", ITERATIONS, || {
        unsafe { SetEvent(event) };
    }));

    unsafe { ResetEvent(event) };
    timings.push(time_loop("set_reset_event", ITERATIONS, || unsafe {
        SetEvent(event);
        ResetEvent(event);
    }));

    unsafe { SetEvent(event) };
    timings.push(time_loop("wait_zero_signalled", ITERATIONS, || {
        unsafe { WaitForSingleObject(event, 0) };
    }));
    unsafe {
        ResetEvent(event);
        CloseHandle(event);
    }

    // The syscall the doorbell would be amortised against. Far fewer
    // iterations: this one is a real kernel transition. `submit_and_wait(0)`
    // asks for no completions, so it returns without blocking and measures the
    // transition rather than any I/O.
    let submit_nanos = ioring::Ring::new().map(|ring| {
        const SUBMIT_ITERATIONS: u32 = 20_000;
        let timing = time_loop("submit_io_ring_empty", SUBMIT_ITERATIONS, || {
            let _ = ring.submit_and_wait(0);
        });
        timings.push(timing);
        timing.nanos_per_op
    });

    Observation {
        timings,
        submit_nanos,
    }
}

/// Keeps the doorbell's own wake path honest: a consumer that actually parks
/// and is woken measures something the zero-timeout poll above does not.
///
/// Reported separately because it is a two-thread measurement and therefore
/// noisier than the single-threaded loops. The number is a full **round trip**
/// -- wake the peer, park, be woken -- not a single transition, so it is an
/// upper bound on what one wakeup costs rather than the cost itself.
///
/// # Why the handshake alternates strictly
///
/// The obvious version -- one thread calling `SetEvent` in a loop while the
/// other calls `WaitForSingleObject` -- **deadlocks**, and did when this probe
/// was first written. An auto-reset event does not count signals: two arriving
/// before one wait collapse into one, the waiter's count never catches up, and
/// it blocks on `INFINITE` for ever. Two events used as ping and pong force
/// strict alternation, so no signal can be lost.
///
/// Every wait is nevertheless bounded. A probe that can hang is a probe that
/// can hang a build, and the deadlock above is exactly how that happens; a
/// timeout turns it into a reported anomaly instead.
///
/// Returns `None` if the handshake ever timed out, because a partial run's
/// average would be meaningless.
#[must_use]
pub fn measure_park_and_wake(rounds: u32) -> Option<f64> {
    const WAIT_TIMEOUT_MS: u32 = 5_000;

    // SAFETY: two auto-reset, initially-unsignalled, unnamed events.
    let ping: HANDLE = unsafe { CreateEventW(std::ptr::null(), 0, 0, std::ptr::null()) };
    let pong: HANDLE = unsafe { CreateEventW(std::ptr::null(), 0, 0, std::ptr::null()) };
    assert!(!ping.is_null() && !pong.is_null(), "CreateEventW failed");

    let (ping_addr, pong_addr) = (ping as usize, pong as usize);
    let peer = std::thread::spawn(move || {
        let (ping, pong) = (ping_addr as HANDLE, pong_addr as HANDLE);
        for _ in 0..rounds {
            // SAFETY: both handles outlive this thread, which is joined below.
            let waited = unsafe { WaitForSingleObject(ping, WAIT_TIMEOUT_MS) };
            if waited != WAIT_OBJECT_0 {
                return false;
            }
            unsafe { SetEvent(pong) };
        }
        true
    });

    let mut ok = true;
    let start = Instant::now();
    for _ in 0..rounds {
        // SAFETY: both handles are live for the whole loop.
        unsafe { SetEvent(ping) };
        if unsafe { WaitForSingleObject(pong, WAIT_TIMEOUT_MS) } != WAIT_OBJECT_0 {
            ok = false;
            break;
        }
    }
    let elapsed = start.elapsed();

    let peer_ok = peer.join().unwrap_or(false);
    // SAFETY: the peer has been joined, so nothing else holds these.
    unsafe {
        CloseHandle(ping);
        CloseHandle(pong);
    }

    (ok && peer_ok).then(|| elapsed.as_nanos() as f64 / f64::from(rounds))
}
