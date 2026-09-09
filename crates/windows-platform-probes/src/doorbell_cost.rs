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
//! not do that.** An empty submission carries no work, so whatever it costs is
//! not the denominator that question needs, and "the doorbell is 210% of a
//! syscall" would be a confident wrong answer whatever the number turned out
//! to be.
//!
//! Whether it even reaches the kernel is host-dependent and the binary decides
//! it per run rather than asserting it. On the Snapdragon X2 (ARM64) development machine it came in
//! at ~79 ns, far below that machine's own syscalls, which reads as
//! short-circuiting in user mode when there is nothing queued; on an x86_64
//! host measured during review it was 216 ns, sitting among that host's 206 ns
//! `SetEvent` and 280 ns satisfied wait, where nothing supports the claim. The
//! argument above needs neither reading, which is why it is stated over the
//! work carried rather than over the transition.
//! The honest denominator is the cost of the real work a submission carries,
//! which this probe deliberately does not measure -- so it reports the absolute
//! costs and the *batching* arithmetic instead, and leaves the ratio alone.
//! [`Observation::doorbell_over_empty_submit`] is retained only because the raw
//! fact is worth recording; it is named for its own denominator, and its own
//! documentation repeats this warning.

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
    /// as one.** An empty submit carries no work, so this ratio has a
    /// denominator that measures nothing the design cares about. It is exposed
    /// because the raw fact is worth recording across hosts -- a machine where
    /// the empty submit is *expensive* would itself be a finding -- not because
    /// dividing by it answers anything.
    ///
    /// The argument rests on *carries no work*, which holds everywhere, and no
    /// longer on *does not enter the kernel*, which does not. That read "an
    /// empty submit does not appear to enter the kernel, so this ratio has a
    /// denominator that is not a syscall" -- a reading from the Snapdragon X2
    /// (ARM64) development machine (~79 ns) stated as a general fact. The
    /// binary decides it per host, and on an x86_64 machine measured during
    /// review it printed the opposite, the
    /// empty submit landing at 216 ns among that probe's own 206 ns syscalls.
    #[must_use]
    pub fn doorbell_over_empty_submit(&self) -> Option<f64> {
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
///
/// Panics, too, if any timed call fails -- `SetEvent`, `ResetEvent`, a
/// zero-timeout wait that does not observe the event as signalled, or
/// `SubmitIoRing`. This is deliberately loud. Each of those still costs a
/// measurable transition when it fails, so a probe that swallowed the error
/// would report a plausible nanosecond figure for an operation that did not
/// happen, which is worse than reporting nothing.
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

    // THE RULE: every call that returns a status has that status checked,
    // inside the timed region. No exceptions, and no per-call-site argument
    // about whether this one is worth it.
    //
    // It is a flat rule on purpose. The previous version checked only
    // `SubmitIoRing`, with a well-argued note there explaining why discarding a
    // status lets a failing call report a plausible time for an operation that
    // never happened -- and left the three event loops above it discarding
    // their `BOOL`s. The argument was correct and got applied to the one call
    // that looked expensive enough to deserve it. That is how the cheap calls
    // get missed: "trivial enough not to check" is not a property of the call,
    // it is a property of how hard anyone looked at it.
    //
    // Both halves of that were measured rather than argued, on the x86_64
    // review host, by running this probe against a deliberately invalid handle
    // so that every event call fails.
    //
    // What the unchecked version reported, in ns/op, against the true figures:
    //
    //     set_event_already_signalled   212.7   (true 205)
    //     set_reset_event               422.9   (true 531)
    //     wait_zero_signalled           231.5   (true 280)
    //
    // Not one of those looks wrong. The redundant-`SetEvent` figure is within
    // 4% of the real one, and a reader would have taken the whole set as
    // evidence and drawn the doorbell-versus-build conclusion from a run in
    // which no event operation ever succeeded. A failing syscall is not cheap
    // enough to be conspicuous -- that is the entire hazard.
    //
    // The checks cost nothing detectable: with them in place the same host
    // reports 204-206, 528-534 and 280.3-280.7 across three runs, which is the
    // run-to-run spread and not a shift. If a check ever does cost enough to
    // distort a figure, that will show up as data and can be tuned then, at
    // that site, with the evidence in hand. Until then the rule does not bend
    // to an estimate.
    //
    // `atomic_fetch_add` above is not an exception: `fetch_add` returns no
    // status, so there is nothing to check.

    // Leave it signalled, so every call in the next loop is redundant.
    assert!(unsafe { SetEvent(event) } != 0, "SetEvent failed");
    timings.push(time_loop("set_event_already_signalled", ITERATIONS, || {
        assert!(
            unsafe { SetEvent(event) } != 0,
            "SetEvent on an already-signalled event failed"
        );
    }));

    assert!(unsafe { ResetEvent(event) } != 0, "ResetEvent failed");
    timings.push(time_loop("set_reset_event", ITERATIONS, || unsafe {
        assert!(SetEvent(event) != 0, "SetEvent failed mid-cycle");
        assert!(ResetEvent(event) != 0, "ResetEvent failed mid-cycle");
    }));

    assert!(unsafe { SetEvent(event) } != 0, "SetEvent failed");
    timings.push(time_loop("wait_zero_signalled", ITERATIONS, || {
        // `assert_eq`, not "did not fail". The label says *satisfied* wait, and
        // `WAIT_TIMEOUT` is a successful return that times a different path --
        // an unsatisfied poll, which is the cheaper one and would flatter the
        // figure.
        assert_eq!(
            unsafe { WaitForSingleObject(event, 0) },
            WAIT_OBJECT_0,
            "a zero-timeout wait did not observe the event as signalled"
        );
    }));
    unsafe {
        assert!(ResetEvent(event) != 0, "ResetEvent failed");
        // Checked as a post-condition: a close that fails means the handle was
        // already invalid, which retroactively discredits every figure above
        // it. That is not theoretical -- in the bad-handle run described above,
        // with every other check stripped out, this was the one that caught it.
        // It is the backstop for a handle that goes bad in a way no individual
        // call happens to report.
        assert!(CloseHandle(event) != 0, "CloseHandle failed");
    }

    // The syscall the doorbell would be amortised against. Far fewer
    // iterations: this one is a real kernel transition. `submit_and_wait(0)`
    // asks for no completions, so it returns without blocking and measures the
    // transition rather than any I/O.
    let submit_nanos = ioring::Ring::new().map(|ring| {
        const SUBMIT_ITERATIONS: u32 = 20_000;
        let timing = time_loop("submit_io_ring_empty", SUBMIT_ITERATIONS, || {
            // Both halves of the answer are checked, inside the timed region.
            // Discarding them let a host where `SubmitIoRing` fails produce a
            // perfectly plausible timing -- a failing call still costs a
            // measurable transition -- which the report then read as the cost of
            // a successful empty submission. That is the failure mode this whole
            // crate exists to avoid: a number that looks like evidence and is
            // not. The `submitted` count is checked too, because a call that
            // succeeded while submitting entries did not measure what the label
            // says it measured.
            //
            // Inside the loop, not outside it: a check after the fact would let
            // the timing be taken before anything established it was valid.
            //
            // Every loop above now does the same, under the flat rule stated
            // there. This note came first and for a while was the only one,
            // which is the whole reason the rule is now flat rather than
            // argued per site.
            let (hr, submitted) = ring.submit_and_wait(0);
            assert!(hr >= 0, "SubmitIoRing(0) failed: {hr:#010x}");
            assert_eq!(submitted, 0, "SubmitIoRing(0) submitted entries");
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
/// average would be meaningless -- and for the same reason if `rounds` is zero,
/// which has no average at all rather than an average of nothing.
#[must_use]
pub fn measure_park_and_wake(rounds: u32) -> Option<f64> {
    const WAIT_TIMEOUT_MS: u32 = 5_000;

    // An empty sample has no average, and the arithmetic below would not say
    // so: no round runs, so the elapsed time is zero, and `0.0 / 0.0` is `NaN`
    // wrapped in the `Some` this function documents as a meaningful number. A
    // caller comparing that against a threshold gets `false` from every
    // comparison and no indication why.
    if rounds == 0 {
        return None;
    }

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
