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

/// How long a probe waits for a pool callback before calling it absent.
///
/// Generous, because these run alongside the rest of the suite on a machine
/// that may be loaded: the question is whether the record exists at all, not
/// how promptly it arrived.
const PROBE_BOUND: Duration = Duration::from_secs(5);

/// Every pool-invoked functor in this crate records both of its ends, and so
/// does every step the delivery investigation reads between them.
///
/// This is what makes a silent interval in a capture readable. An entry with
/// no exit says a callback is still inside the closure; no entry at all says
/// the pool never dispatched. Those are different findings, and only the exit
/// records separate them -- so a deleted exit call site would quietly turn the
/// second into the first. The same argument covers the re-arm, whose entry is
/// recorded before a lock it can block on, and the drop stages, which bracket
/// the drain.
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
#[test]
fn every_pool_trampoline_records_both_of_its_ends() {
    // `io` is exercised by `crate::io::tests`, which already has an endpoint
    // and a real overlapped read; asserting there costs two lines rather than
    // a second copy of that setup here.
    let expected: [(&str, &[&str]); 4] = [
        (
            "wait",
            &[
                "created",
                "armed",
                "trampoline-entered",
                "trampoline-left",
                // The re-arm the exercise drives from the pool thread.
                "rearm-entered",
                "rearm-left",
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
            ],
        ),
        ("timer", &["trampoline-entered", "trampoline-left"]),
        ("timer-periodic", &["trampoline-entered", "trampoline-left"]),
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

/// One one-shot timer firing.
fn exercise_a_timer() {
    use crate::timer::ThreadpoolTimer;

    let (tx, rx) = mpsc::channel();
    let tx = std::sync::Mutex::new(tx);
    let timer = ThreadpoolTimer::new(
        move |_| {
            if let Ok(tx) = tx.lock() {
                let _ = tx.send(());
            }
        },
        None,
    )
    .expect("create a probe timer");
    timer.set_after(Duration::from_millis(1));
    rx.recv_timeout(PROBE_BOUND).expect("the timer fired");
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
