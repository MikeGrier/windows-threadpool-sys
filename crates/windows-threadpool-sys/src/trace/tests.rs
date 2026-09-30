// Copyright (c) Mike Grier
//! Tests for the trace facility.
//!
//! These run in both builds. Without the `trace` feature every entry point is
//! a no-op, and asserting *that* is the point: a call site left in shipping
//! code must cost nothing and must not misreport.
//!
//! The eviction policy is tested against [`Buffer`] directly rather than
//! through the global one. Two reasons, and the second is the binding one:
//! a capacity of four is reachable where eight thousand is tedious, and
//! filling the real buffer would evict every record the rest of the suite had
//! just taken.

use std::sync::mpsc;
use std::time::Duration;

use super::{Buffer, Record, counted, dump, enabled, wants};

/// A record distinguishable from its neighbours by its `a` slot.
fn record(a: u64) -> Record {
    Record {
        at: Duration::from_micros(a),
        thread: 1,
        target: "test",
        event: "filler",
        a,
        b: 0,
    }
}

/// The `a` slots still held, oldest first.
fn held(buffer: &Buffer) -> Vec<u64> {
    buffer.records.iter().map(|record| record.a).collect()
}

#[test]
fn a_buffer_below_its_capacity_keeps_everything_and_drops_nothing() {
    let mut buffer = Buffer::with_capacity(4);
    for a in 0..4 {
        assert!(
            !buffer.push(record(a), 4),
            "a push that fits is not an eviction"
        );
    }
    assert_eq!(held(&buffer), vec![0, 1, 2, 3]);
    assert_eq!(buffer.dropped, 0);
    assert!(
        !buffer.announced,
        "nothing has been lost, so nothing may be announced"
    );
}

#[test]
fn a_full_buffer_evicts_the_oldest_and_keeps_the_newest() {
    let mut buffer = Buffer::with_capacity(4);
    for a in 0..7 {
        buffer.push(record(a), 4);
    }
    assert_eq!(
        held(&buffer),
        vec![3, 4, 5, 6],
        "the window must slide, not stop: a buffer that stopped recording when full would \
         report the same length and lose the end of the run instead of the start"
    );
    assert_eq!(buffer.dropped, 3, "one drop per push past the capacity");
}

#[test]
fn only_the_first_eviction_is_announced() {
    let mut buffer = Buffer::with_capacity(2);
    buffer.push(record(0), 2);
    buffer.push(record(1), 2);
    assert!(
        buffer.push(record(2), 2),
        "the first eviction is the one worth reporting"
    );
    for a in 3..10 {
        assert!(
            !buffer.push(record(a), 2),
            "reporting every later eviction would put a formatted line on the traced path \
             once per record, which is what this facility exists not to do"
        );
    }
    assert_eq!(buffer.dropped, 8, "silence is not the same as not counting");
}

#[test]
fn a_cleared_buffer_will_announce_an_overflow_again() {
    let mut buffer = Buffer::with_capacity(1);
    buffer.push(record(0), 1);
    assert!(buffer.push(record(1), 1));
    buffer.clear();
    assert!(held(&buffer).is_empty());
    assert_eq!(buffer.dropped, 0);
    buffer.push(record(2), 1);
    assert!(
        buffer.push(record(3), 1),
        "a clear starts a new capture, and a new capture that overflows is as worth \
         reporting as the first one was"
    );
}

#[test]
fn a_zero_capacity_buffer_stores_nothing_rather_than_panicking() {
    // Degenerate, and unreachable through the global buffer -- but the
    // eviction branch reaches `remove(0)`, which panics on an empty vector, so
    // the case is defined here rather than left to be discovered.
    let mut buffer = Buffer::with_capacity(0);
    assert!(
        buffer.push(record(0), 0),
        "the first loss is still announced"
    );
    assert!(!buffer.push(record(1), 0));
    assert!(held(&buffer).is_empty());
    assert_eq!(buffer.dropped, 2, "everything offered was dropped");
}

#[test]
fn a_build_without_the_feature_reports_nothing_and_says_so() {
    if cfg!(feature = "trace") {
        return;
    }
    assert!(!enabled(), "tracing cannot be on without the feature");
    assert!(
        !wants("anything"),
        "no target is traced without the feature"
    );
    assert!(
        dump().contains("without the `trace` feature"),
        "a dump must say why it is empty rather than look like a clean run -- an empty trace and \
         a trace that was never compiled in are very different findings"
    );
}

#[test]
fn recording_is_inert_without_the_feature() {
    if cfg!(feature = "trace") {
        return;
    }
    // The call must compile and do nothing. If this ever starts recording,
    // the shipping build has acquired a lock and an allocation on a path that
    // is meant to carry neither.
    crate::trace_record!("test", "inert", 1, 2);
    assert!(!dump().contains("inert"));
}

#[test]
fn the_filter_narrows_to_the_targets_named() {
    if !cfg!(feature = "trace") {
        return;
    }
    // `wants` is driven by the process environment, which is read once and
    // cached, so this asserts the relationship that holds whatever the
    // variable is set to rather than mutating it: a target that is wanted
    // implies tracing is on at all.
    for target in ["wait", "delivery", "something-else"] {
        if wants(target) {
            assert!(
                enabled(),
                "a wanted target implies the trace is enabled: {target}"
            );
        }
    }
}

/// The exception observer notes a first-chance exception.
///
/// `OutputDebugStringA` raises `DBG_PRINTEXCEPTION_C` and catches it itself,
/// so it is a genuine first-chance exception that changes nothing -- exactly
/// the population this observer exists to see, and one a debugger view would
/// hide behind its own handling.
///
/// **Raised repeatedly on purpose.** The handler records with `try_lock` and
/// drops the record rather than block, which is what makes it safe to run on a
/// thread that may already be inside the trace holding that lock. Under the
/// full suite with every target enabled the buffer lock is hot enough that a
/// single raise really can be lost -- this test asserted on one raise and was
/// flaky for exactly that reason. Asserting that *at least one of many* landed
/// keeps the guarantee that matters (the handler is installed and records)
/// without pretending a designed-in loss does not happen.
///
/// Env-gated like the other guards here, and for the same reason: the filter
/// is read once per process, so a test cannot set it without racing every
/// other test in the binary. Feature-gated too, because without `trace` there
/// is no handler to exercise and the binding it probes with is not compiled.
#[cfg(feature = "trace")]
#[test]
fn the_exception_observer_notes_a_first_chance_exception() {
    if !wants("exception") {
        return;
    }
    /// Enough that losing every one to lock contention is not a thing that
    /// happens, few enough to stay instant.
    const RAISES: usize = 64;

    let before = counted("exception", "raised");
    let text = c"windows-threadpool-sys exception observer probe";
    for _ in 0..RAISES {
        // SAFETY: a valid NUL-terminated string, live for the duration of the call.
        unsafe {
            windows_sys::Win32::System::Diagnostics::Debug::OutputDebugStringA(text.as_ptr().cast())
        };
        if counted("exception", "raised") > before {
            return;
        }
    }
    panic!(
        "the trace is narrowed to `exception` and {RAISES} first-chance exceptions have just been \
         raised with none recorded, so either the handler is not installed or it is losing every \
         record"
    );
}

/// How long a probe waits for a pool callback before calling it absent.
///
/// Generous, because these run alongside the rest of the suite on a machine
/// that may be loaded: the question is whether the record exists at all, not
/// how promptly it arrived.
const PROBE_BOUND: Duration = Duration::from_secs(5);

/// Every pool object in this crate records its whole life: its creation, every
/// arming or submission that establishes it, both ends of every callback it
/// invokes, and its teardown.
///
/// This is what makes a silent interval in a capture readable, and the
/// establishment half is not optional decoration. An entry with no exit says a
/// callback is still inside the closure; no entry at all says the pool never
/// dispatched. Those are different findings, and only the exit records separate
/// them. In exactly the same way, a `created` with no `armed` says an object
/// was never established, while an `armed` with no callback says it was
/// established and not dispatched -- and an investigation into *why a callback
/// is late* cannot tell those apart without both. That gap is what `M-T1.1`
/// closed for the timer and the I/O object, which until then recorded only
/// their firings.
///
/// **It asserts only when the process environment has narrowed the trace.**
/// The filter is read once per process and cached, so a test cannot set it
/// without racing every other test in the binary. Run it as:
///
/// ```text
/// $env:WINDOWS_THREADPOOL_TRACE = '*'
/// cargo test -p windows-threadpool-sys --features trace
/// ```
///
/// Anything less returns having checked nothing, which is stated here rather
/// than left for a reader to infer from a green run.
///
/// **The assertion is one-sided on purpose.** Other tests in the binary record
/// under these targets concurrently, so no count here is attributable to this
/// test. What a one-sided assertion does catch is the case it exists for: a
/// missing call site makes its event absent from the entire process, not
/// merely scarce.
///
/// **It is also per-event, not per-call-site**, which matters for the two
/// events emitted from two places: `timer`'s `rearm-requested`, from
/// `rearm_after` and `rearm_at`, and `io`'s `start-cancelled`, from the
/// inline-completion and issue-failure arms of `submit`. Losing one of a pair
/// leaves the event present and this test green. The exercises below reach both
/// `rearm-requested` sites so neither is merely written; `io`'s inline
/// completion is not reachable from a unit test here and is covered by the
/// integration suite, where this assertion does not run.
#[test]
fn every_pool_object_records_its_creation_establishment_callbacks_and_teardown() {
    // `io` is exercised by `crate::io::tests`, which already has an endpoint
    // and a real overlapped read; asserting there costs a few lines rather than
    // a second copy of that setup here.
    let expected: [(&str, &[&str]); 6] = [
        (
            "wait",
            &[
                "created",
                "armed",
                "disarmed",
                "trampoline-entered",
                "trampoline-left",
                // The re-arm the exercise drives from the pool thread.
                "rearm-entered",
                "rearm-left",
                "suppress-and-disarm",
                // The drop stages, whose gaps are what a stalled teardown
                // would show up as.
                "drop-begin",
                "drop-drained",
                "drop-closed",
            ],
        ),
        (
            "work",
            &[
                "created",
                "submitted",
                "trampoline-entered",
                "trampoline-left",
                "drop-begin",
                "drop-drained",
                "drop-closed",
            ],
        ),
        (
            "timer",
            &[
                "created",
                "armed",
                "disarmed",
                "trampoline-entered",
                "trampoline-left",
                // The deferred re-arm's three moments: the callback asking,
                // the trampoline acting on it after the callback returns, and
                // the arming itself.
                "rearm-requested",
                "rearm-entered",
                "rearm-left",
                "suppress-and-disarm",
                "drop-begin",
                "drop-drained",
                "drop-closed",
            ],
        ),
        (
            "timer-periodic",
            &[
                "created",
                "trampoline-entered",
                "trampoline-left",
                "drop-begin",
                "drop-drained",
                "drop-closed",
            ],
        ),
        // The Win32 calls themselves, bracketed so a call that blocked is an
        // interval rather than a late timestamp. Asserted on both sides,
        // because an enter with no leave is the finding this exists for and a
        // missing leave would make every such call look instantaneous.
        (
            "syscall-enter",
            &[
                "CreateThreadpoolWait",
                "SetThreadpoolWait",
                "WaitForThreadpoolWaitCallbacks(cancel)",
                "CloseThreadpoolWait",
                "CreateThreadpoolWork",
                "SubmitThreadpoolWork",
                "WaitForThreadpoolWorkCallbacks",
                "CloseThreadpoolWork",
                "CreateThreadpoolTimer",
                "SetThreadpoolTimer",
                "WaitForThreadpoolTimerCallbacks(cancel)",
                "CloseThreadpoolTimer",
            ],
        ),
        (
            "syscall-leave",
            &[
                "CreateThreadpoolWait",
                "SetThreadpoolWait",
                "WaitForThreadpoolWaitCallbacks(cancel)",
                "CloseThreadpoolWait",
                "CreateThreadpoolWork",
                "SubmitThreadpoolWork",
                "WaitForThreadpoolWorkCallbacks",
                "CloseThreadpoolWork",
                "CreateThreadpoolTimer",
                "SetThreadpoolTimer",
                "WaitForThreadpoolTimerCallbacks(cancel)",
                "CloseThreadpoolTimer",
            ],
        ),
    ];
    if !expected.iter().any(|(target, _)| wants(target)) {
        return;
    }

    exercise_a_wait();
    exercise_a_work_item();
    exercise_a_timer();
    exercise_a_periodic_timer();

    for (target, events) in expected {
        if !wants(target) {
            continue;
        }
        for event in events {
            assert!(
                counted(target, event) > 0,
                "`{target}` recorded no `{event}`; the trace is narrowed to it and the \
                 corresponding step has just run, so the call site is missing"
            );
        }
    }
}

/// One wait activation, which also drives one re-arm from the pool thread.
fn exercise_a_wait() {
    use crate::wait::{ThreadpoolWait, WaitableHandle};

    // Auto-reset, so the re-arm below does not re-enter the callback on a
    // signal that is still standing.
    let event = WaitableHandle::event(false, false).expect("create a probe event");
    let (tx, rx) = mpsc::channel();
    let tx = std::sync::Mutex::new(tx);
    let wait = ThreadpoolWait::new(
        event,
        move |activation| {
            // Re-armed before the send, so the re-arm's own records are in the
            // trace by the time this test reads it.
            activation.rearm(None);
            if let Ok(tx) = tx.lock() {
                let _ = tx.send(());
            }
        },
        None,
    )
    .expect("create a probe wait");
    wait.arm(None);
    // SAFETY: the wait owns the event, so the handle is open here.
    unsafe {
        windows_sys::Win32::System::Threading::SetEvent(
            std::os::windows::io::AsRawHandle::as_raw_handle(&wait.handle()),
        )
    };
    rx.recv_timeout(PROBE_BOUND).expect("the wait callback ran");
    wait.stop_and_drain();
}

/// One work-item invocation.
fn exercise_a_work_item() {
    use crate::work::ThreadpoolWork;

    let (tx, rx) = mpsc::channel();
    let tx = std::sync::Mutex::new(tx);
    let work = ThreadpoolWork::new(
        move || {
            if let Ok(tx) = tx.lock() {
                let _ = tx.send(());
            }
        },
        None,
    )
    .expect("create a probe work item");
    work.submit();
    rx.recv_timeout(PROBE_BOUND).expect("the work callback ran");
    work.wait();
}

/// One one-shot timer firing, which also drives one deferred re-arm.
fn exercise_a_timer() {
    use crate::timer::ThreadpoolTimer;

    let (tx, rx) = mpsc::channel();
    let tx = std::sync::Mutex::new(tx);
    let firings = std::sync::atomic::AtomicUsize::new(0);
    let timer = ThreadpoolTimer::new(
        move |firing| {
            // Re-arm on the first firing and report on the second. Waiting for
            // the *second* is what makes the re-arm's records certain to be in
            // the trace by the time this test reads it: a second firing can
            // only happen if the deferred re-arm was requested, applied, and
            // armed. Reporting on the first would race the trampoline, which
            // applies the request after the callback returns.
            if firings.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                firing.rearm_after(Duration::from_millis(1));
                return;
            }
            // The second request goes through the other entry point, so both
            // `rearm-requested` call sites are executed rather than only the
            // one that happened to be written first.
            if firings.load(std::sync::atomic::Ordering::SeqCst) == 1 {
                firing.rearm_at(std::time::SystemTime::now() + Duration::from_millis(1));
                return;
            }
            if let Ok(tx) = tx.lock() {
                let _ = tx.send(());
            }
        },
        None,
    )
    .expect("create a probe timer");
    timer.set_after(Duration::from_millis(1));
    rx.recv_timeout(PROBE_BOUND)
        .expect("the timer fired, re-armed twice, and fired again");
    timer.stop_and_drain();
}

/// One periodic-timer tick.
fn exercise_a_periodic_timer() {
    use crate::timer::ThreadpoolPeriodicTimer;

    let (tx, rx) = mpsc::channel();
    let tx = std::sync::Mutex::new(tx);
    let timer = ThreadpoolPeriodicTimer::new(
        Duration::from_millis(1),
        move |_| {
            if let Ok(tx) = tx.lock() {
                let _ = tx.send(());
            }
        },
        None,
    )
    .expect("create a probe periodic timer");
    timer.start();
    rx.recv_timeout(PROBE_BOUND)
        .expect("the periodic timer ticked");
    // Drained rather than merely stopped: ticks keep arriving until it is, and
    // this callback sends into a channel the test is about to drop.
    timer.stop_and_drain();
}

/// The stub recogniser is what makes patching `ntdll` safe, so it is asserted
/// in **both** directions: that it accepts the shape, and that it rejects
/// everything else. A recogniser tested only on things it should accept would
/// pass just as well if it accepted everything, which is the failure mode that
/// matters -- accepting a non-stub means planting a jump over instructions
/// that were never decoded.
///
/// The hermetic cases pin the pattern; the live `ntdll` case is the one that
/// says the pattern still describes this machine's Windows.
#[cfg(feature = "trace")]
#[test]
fn the_stub_recogniser_accepts_the_shape_and_rejects_everything_else() {
    use super::hook::{is_syscall_stub, stub_entry};

    // mov r10,rcx / mov eax,0x1E6 / test byte ptr [7FFE0308h],1
    let genuine: [u8; 16] = [
        0x4C, 0x8B, 0xD1, 0xB8, 0xE6, 0x01, 0x00, 0x00, 0xF6, 0x04, 0x25, 0x08, 0x03, 0xFE, 0x7F,
        0x01,
    ];
    // SAFETY: sixteen readable bytes, which is all the recogniser reads.
    assert!(
        unsafe { is_syscall_stub(genuine.as_ptr()) },
        "the canonical stub shape must be accepted, or nothing can ever be hooked"
    );

    for spoiled_at in [0, 1, 2, 3, 8, 9, 10, 11, 12, 13, 14, 15] {
        let mut altered = genuine;
        altered[spoiled_at] ^= 0xFF;
        // SAFETY: as above.
        assert!(
            !unsafe { is_syscall_stub(altered.as_ptr()) },
            "byte {spoiled_at} is part of the fixed prefix, so changing it must be a refusal"
        );
    }

    // The four system-call-number bytes are the one wildcard, and must stay
    // one: a recogniser that pinned them would refuse every stub but the one
    // it was written against.
    for wildcard_at in 4..8 {
        let mut altered = genuine;
        altered[wildcard_at] ^= 0xFF;
        // SAFETY: as above.
        assert!(
            unsafe { is_syscall_stub(altered.as_ptr()) },
            "byte {wildcard_at} is the system call number and must not be part of the match"
        );
    }

    let live = stub_entry("selftest").expect("ntdll exports the self-test stub");
    // SAFETY: an exported entry point has at least sixteen readable bytes.
    assert!(
        unsafe { is_syscall_stub(live) },
        "the pattern no longer describes a real ntdll stub on this build, so every install would \
         refuse -- the shape has changed and this module needs revisiting"
    );
}

/// A hooked stub records both ends of every call **and still performs the
/// system call it displaced**.
///
/// Both halves matter and they fail differently. A hook that records but
/// breaks the call would leave the process quietly wrong; a trampoline that
/// works but records nothing would leave an investigation reading an empty
/// capture and concluding the call never happened -- which is exactly the
/// inference this facility exists to support, so a silent hook is worse than
/// no hook.
///
/// Uses the table's self-test entry rather than a worker-factory stub: the
/// patch is never removed, so hooking a busy stub here would follow every
/// later test in the process.
#[cfg(feature = "trace")]
#[test]
fn a_hooked_stub_records_both_ends_and_still_performs_its_syscall() {
    use super::hook::{call_selftest, factory_handle, fired, install_by_label};

    install_by_label("selftest").expect("the self-test stub is hookable");

    // Counted rather than read out of the trace, so this runs on every machine
    // rather than only where `WINDOWS_THREADPOOL_TRACE` happens to be set. The
    // record assertions below are additional, and are skipped when the trace is
    // not armed -- but the fired count is not, because a guard that is silently
    // inert wherever the environment is unset is not a guard.
    let before_fired = fired("selftest");
    let before_enter = counted("wfactory", "selftest-enter");
    let before_leave = counted("wfactory", "selftest-leave");

    // The call whose result proves the trampoline: a working system call fills
    // all three values and reports success, so a trampoline that jumped
    // somewhere useless cannot produce this.
    let (status, minimum, maximum, current) = call_selftest();
    assert!(status >= 0, "the displaced system call must still succeed");
    assert!(
        minimum > 0 && maximum > 0 && current > 0,
        "the system call must still write its three out-parameters, got \
         minimum={minimum} maximum={maximum} current={current}"
    );
    assert!(
        maximum <= current && current <= minimum,
        "the values must still be the kernel's own, ordered finest to coarsest: \
         maximum={maximum} current={current} minimum={minimum}"
    );

    assert!(
        fired("selftest") > before_fired,
        "the planted jump must actually be reached: the call above went somewhere, and if it \
         was not through the hook then nothing is installed"
    );

    assert_eq!(
        factory_handle(),
        0,
        "the self-test stub's first argument is an out-pointer, not a worker factory handle, \
         so it must not be mistaken for one -- a hook that learned a handle from the wrong \
         call would leave the factory counters describing whatever that pointer happened to be"
    );

    if wants("wfactory") {
        assert!(
            counted("wfactory", "selftest-enter") > before_enter,
            "the hook must record entering the call"
        );
        assert!(
            counted("wfactory", "selftest-leave") > before_leave,
            "the hook must record leaving it, or a call that never returned would be \
             indistinguishable from one that did"
        );
    }
}

/// An install refuses, and leaves `ntdll` alone, when the name is not there.
#[cfg(feature = "trace")]
#[test]
fn an_install_of_an_unknown_label_refuses_rather_than_patching_something() {
    use super::hook::{Refusal, install_by_label};

    assert_eq!(
        install_by_label("no-such-hook-label"),
        Err(Refusal::NotFound),
        "an unrecognised label must refuse; silently patching index zero would be a \
         catastrophic misreading of a typo"
    );
}

/// The factory scan finds this process's worker factory, and what it reports
/// is the pool's own state rather than a misread of some other object.
///
/// The scan matters more than the hooks it backs up. A stalled process is
/// precisely one in which no hooked call has fired, so the handle a hook would
/// have learned is exactly the handle that is missing at the moment it is
/// needed -- and this asks the question without needing one, and without
/// patching anything.
///
/// The assertions pin the layout, not just the call. A wrong `Basic` would
/// still let the query succeed while placing the counts over the timeouts or
/// the padding, so the guard requires the numbers to be consistent with a pool
/// that has just run a callback: at least one worker, and no more waiting than
/// exist.
#[cfg(feature = "trace")]
#[test]
fn the_factory_scan_finds_the_pool_and_reads_plausible_counts() {
    use super::hook::probe_factory;
    use crate::work::ThreadpoolWork;

    // Force the default pool into existence and make it dispatch, so there is
    // a factory to find and it has at least one worker.
    let (tx, rx) = mpsc::channel();
    let tx = std::sync::Mutex::new(tx);
    let work = ThreadpoolWork::new(
        move || {
            if let Ok(tx) = tx.lock() {
                let _ = tx.send(());
            }
        },
        None,
    )
    .expect("create the work item");
    work.submit();
    rx.recv_timeout(PROBE_BOUND)
        .expect("the default pool ran the callback");

    let (total, waiting, pending) =
        probe_factory().expect("the scan must find the worker factory of a process that has one");

    assert!(
        total >= 1,
        "a pool that has just run a callback has at least one worker, got total={total}"
    );
    assert!(
        waiting <= total,
        "more workers waiting than exist means the layout is misread: waiting={waiting} \
         total={total}"
    );
    assert!(
        total < 10_000 && pending < 10_000,
        "these are small counts for a test process; total={total} pending={pending} reads like \
         a timeout or a pointer landing in the fields the layout names"
    );
}

/// The trace's static initialiser really runs before `main`.
///
/// The whole point of `.CRT$XCU` here is timing: hooks installed on the first
/// traced call land inside the first test, which in the investigation this was
/// built for is already too late to see the thing being investigated. So the
/// property worth guarding is not "the hooks work" -- that has its own test --
/// but "the arrangement to run them early was not silently discarded".
///
/// It is a live risk rather than a theoretical one. A static that nothing
/// references is exactly what a linker may drop; `#[used]` is what asks it not
/// to; and if that ask stops working the build still succeeds and the
/// instrument quietly goes back to arriving late. There is no compile error to
/// catch it, so there is a test.
#[cfg(feature = "trace")]
#[test]
fn the_trace_arms_itself_before_main() {
    assert!(
        super::armed_before_main(),
        "the .CRT$XCU initialiser did not run, so the trace armed lazily instead -- hooks will \
         install inside the first test rather than before any thread exists, which is the \
         failure this arrangement exists to prevent"
    );
}

/// Hooking the wait registration records whether the kernel reported the
/// object as already signalled.
///
/// Arming a `ThreadpoolWait` on an event that is **already set** is the case
/// that takes the second delivery path: the kernel queues no completion and
/// reports the fact through `AlreadySignaled` instead. This asserts the hook
/// observes that flag at all -- without it, a capture showing no
/// `associate-already-signalled` record would be ambiguous between "the flag
/// was never set" and "nothing was ever looking".
///
/// Deliberately does not assert *which* value comes back. That is a fact about
/// the kernel's behaviour on the day, and pinning it here would turn an
/// observation into an expectation.
#[cfg(feature = "trace")]
#[test]
fn hooking_the_wait_registration_observes_the_already_signalled_flag() {
    use super::hook::{fired, install_by_label};
    use crate::wait::{ThreadpoolWait, WaitableHandle};

    install_by_label("associate").expect("the wait-registration stub is hookable");
    let before = fired("associate");

    // Signalled at creation, so the association races an object that is
    // already set -- which is the path the flag exists to report.
    let event = WaitableHandle::event(true, true).expect("create a set event");
    let (tx, rx) = mpsc::channel();
    let tx = std::sync::Mutex::new(tx);
    let wait = ThreadpoolWait::new(
        event,
        move |_| {
            if let Ok(tx) = tx.lock() {
                let _ = tx.send(());
            }
        },
        None,
    )
    .expect("create the wait");
    wait.arm(None);
    rx.recv_timeout(PROBE_BOUND)
        .expect("an already-signalled event still reaches the callback");

    assert!(
        fired("associate") > before,
        "arming a wait must go through NtAssociateWaitCompletionPacket, so the hook must have \
         been entered -- if it was not, the registration took a path this facility cannot see"
    );
}

/// The completion-port scan finds a port and reports its depth correctly.
///
/// **The positive control for `M-T5.1`.** That measurement turns on reading a
/// stalled pool's completion-port depth, and its two outcomes send the
/// investigation in opposite directions -- so the failure that matters is not a
/// wrong number but a silent one. A scan that resolved nothing, or that found no
/// port, would report exactly what a genuinely empty port reports, and the
/// conclusion drawn from it would be the opposite of the truth.
///
/// So this builds a port whose depth is *known* rather than inferred, and
/// requires the scan to agree. Both halves are asserted, because they fail
/// differently: that a port is found at all, and that the depth read back is the
/// number of packets posted. Asserting only the first would pass on a misread
/// layout; asserting only the second would pass vacuously on an empty scan.
///
/// The depth is checked against an exact count rather than a range. This process
/// holds other ports -- the default pool has one, and it is deliberately not
/// disturbed here -- so the assertion is that *some* port reports exactly what
/// was posted, not that every port does.
#[cfg(feature = "trace")]
#[test]
fn the_port_scan_finds_a_completion_port_and_reads_its_depth() {
    use super::hook::probe_ports;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::IO::{CreateIoCompletionPort, PostQueuedCompletionStatus};

    /// Packets posted before the scan. More than one so the guard pins the
    /// depth rather than a boolean, and an odd value unlikely to coincide with
    /// whatever another port in the process happens to hold.
    const POSTED: u32 = 7;

    // SAFETY: the documented way to create a standalone completion port.
    let port: HANDLE =
        unsafe { CreateIoCompletionPort(INVALID_HANDLE_VALUE, std::ptr::null_mut(), 0, 1) };
    assert!(!port.is_null(), "create a completion port");

    for i in 0..POSTED {
        // SAFETY: `port` is a live completion port; the packet carries no
        // overlapped pointer, which a queue depth does not interpret.
        let ok = unsafe { PostQueuedCompletionStatus(port, i, 0, std::ptr::null_mut()) };
        assert!(ok != 0, "post packet {i}");
    }

    let seen = probe_ports();
    // SAFETY: nothing else holds this handle and the scan does not retain it.
    unsafe { CloseHandle(port) };

    assert!(
        !seen.is_empty(),
        "the scan must find at least one completion port in a process that just made one; \
         finding none is what a silently-broken probe reports, and it reads identically to \
         an empty port"
    );
    assert!(
        seen.iter().any(|(depth, _)| *depth == POSTED),
        "no port reported the {POSTED} packets that were posted to one of them, so the depth \
         is not being read correctly: saw {seen:?}"
    );
}
