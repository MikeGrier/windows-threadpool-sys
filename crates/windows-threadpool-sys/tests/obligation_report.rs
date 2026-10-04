// Copyright (c) Mike Grier
//! That `Drop` actually emits the undischarged-obligation report, end to end.
//!
//! The trace's filter is fixed by a pre-`main` initialiser, so a test cannot
//! narrow it for itself: the process has to be *launched* with
//! `WINDOWS_THREADPOOL_TRACE` already set. This test therefore re-executes this
//! same binary as a child with the filter set, and asserts from the parent that
//! the child exited cleanly -- the same shape `callback_panic_aborts.rs` uses
//! for a condition it cannot observe in-process.
//!
//! Re-executing is what makes the check run at all. An earlier version asked
//! whether the filter happened to be armed and returned early when it was not,
//! which is every ordinary `cargo test` invocation and every CI run: the body
//! observed nothing and libtest recorded a pass. A check that reports success
//! while measuring nothing is worse than an absent one, and this one was in
//! that state for its whole life. The `eprintln!` it printed first was not a
//! substitute, because nothing reads the stdout of a passing test.
//!
//! The child also runs the half of the contract that `fail-fast` turns into a
//! panic. Dropping an object that still owes a drain is the behaviour under
//! test, so the drop is performed inside `catch_unwind` and the outcome is
//! asserted *against the feature*: with `fail-fast` on it must panic, and with
//! it off it must not. The report is emitted before the panic is raised -- see
//! the ordering in each type's `Drop` -- so the record is countable either way.

#![cfg(all(windows, feature = "trace"))]

use std::process::{Command, Stdio};
use std::time::Duration;
use windows_threadpool_sys::timer::{ThreadpoolPeriodicTimer, ThreadpoolTimer};
use windows_threadpool_sys::wait::{ThreadpoolWait, WaitableHandle};
use windows_threadpool_sys::work::ThreadpoolWork;

/// The variable that marks the child. Absent in the parent.
const CHILD_VAR: &str = "WTPS_OBLIGATION_REPORT_CHILD";

/// The filter the child is launched with, covering every type that reports.
const TRACE_FILTER: &str = "wait,timer,timer-periodic,work";

/// How long the child gets before the parent gives up on it.
const CHILD_TIMEOUT: Duration = Duration::from_secs(60);

/// The child's exit code when an assertion about the report failed.
const ASSERTION_FAILED_EXIT_CODE: i32 = 1;

/// The child's exit code when the trace filter did not reach it.
///
/// Distinct from an assertion failure because it means the *harness* is broken
/// -- the child observed nothing, which is the exact condition this file was
/// rewritten to stop reporting as success.
const FILTER_MISSING_EXIT_CODE: i32 = 111;

/// The tag every type emits when it finds an obligation owed at `Drop`.
///
/// Spelled out rather than imported: the constant is crate-internal, and a
/// consumer filtering a capture has only this string to go on. If the two ever
/// disagree, the crate's own `the_event_tag_is_stable` unit test is what fails.
const TAG: &str = "drop-obligation-owed";

/// How many records so far carry this target and this event.
fn counted(target: &str, event: &str) -> usize {
    windows_threadpool_sys::trace::dump()
        .lines()
        .filter(|line| {
            let mut fields = line.split_whitespace();
            // Elapsed time, then thread id, then the two labels.
            fields.next();
            fields.next();
            fields.next() == Some(target) && fields.next() == Some(event)
        })
        .count()
}

/// Run `body`, returning whether it panicked, with the panic message suppressed.
///
/// The panic is the expected outcome under `fail-fast`, so printing it would put
/// an alarming backtrace in the output of a passing run. The hook is restored
/// immediately afterwards, so a genuine failure later still reports normally.
/// Safe to do here and nowhere else: this is a dedicated child process running
/// one test on one thread, so no sibling can lose its own panic message to this.
fn panicked(body: impl FnOnce()) -> bool {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(body));
    std::panic::set_hook(previous);
    outcome.is_err()
}

/// Arm `target` and discharge it, so `Drop` is left nothing to report.
fn arm_and_discharge(target: &str) {
    match target {
        "wait" => {
            let event = WaitableHandle::event(true, false).expect("event");
            let wait = ThreadpoolWait::new(event, |_| {}, None).expect("wait");
            wait.arm(None);
            wait.stop_and_drain();
        }
        "timer" => {
            let timer = ThreadpoolTimer::new(|_| {}, None).expect("timer");
            timer.set_after(Duration::from_secs(60));
            timer.stop_and_drain();
        }
        "timer-periodic" => {
            let timer = ThreadpoolPeriodicTimer::new(Duration::from_secs(60), |_| {}, None)
                .expect("periodic timer");
            timer.start();
            timer.stop_and_drain();
        }
        "work" => {
            let work = ThreadpoolWork::new(|| {}, None).expect("work");
            work.submit();
            work.wait();
        }
        other => unreachable!("unexpected target {other}"),
    }
}

/// Arm `target` and drop it still owing, so `Drop` performs the drain and says so.
fn arm_and_abandon(target: &str) {
    match target {
        "wait" => {
            let event = WaitableHandle::event(true, false).expect("event");
            let wait = ThreadpoolWait::new(event, |_| {}, None).expect("wait");
            wait.arm(None);
        }
        "timer" => {
            let timer = ThreadpoolTimer::new(|_| {}, None).expect("timer");
            timer.set_after(Duration::from_secs(60));
        }
        "timer-periodic" => {
            let timer = ThreadpoolPeriodicTimer::new(Duration::from_secs(60), |_| {}, None)
                .expect("periodic timer");
            timer.start();
        }
        "work" => {
            let work = ThreadpoolWork::new(|| {}, None).expect("work");
            work.submit();
        }
        other => unreachable!("unexpected target {other}"),
    }
}

/// The measurement, run in the child with the filter armed.
fn check_every_target() {
    let targets = ["wait", "timer", "timer-periodic", "work"];
    for target in targets {
        assert!(
            windows_threadpool_sys::trace::wants(target),
            "the child was launched with {CHILD_VAR} set but the trace is not \
             armed for `{target}`"
        );

        let before = counted(target, TAG);

        arm_and_discharge(target);
        assert_eq!(
            counted(target, TAG),
            before,
            "`{target}` reported an obligation the caller had already discharged"
        );

        // The drop is the behaviour under test, and `fail-fast` turns it into a
        // panic. Assert which of the two happened rather than tolerating either:
        // a feature that is supposed to panic and quietly does not is the same
        // class of defect as this file's old early return.
        let panicked_on_drop = panicked(|| arm_and_abandon(target));
        if cfg!(feature = "fail-fast") {
            assert!(
                panicked_on_drop,
                "`{target}` was dropped still owing a drain under `fail-fast`, \
                 and did not panic"
            );
        } else {
            assert!(
                !panicked_on_drop,
                "`{target}` panicked on drop without the `fail-fast` feature, \
                 which must only report"
            );
        }

        assert_eq!(
            counted(target, TAG),
            before + 1,
            "`{target}` did not report an obligation the caller left to Drop"
        );
    }
}

/// In the child, run the measurement and exit; in the parent, return.
fn dispatch_if_child() {
    if std::env::var(CHILD_VAR).is_err() {
        return;
    }
    if !windows_threadpool_sys::trace::wants("wait") {
        std::process::exit(FILTER_MISSING_EXIT_CODE);
    }
    match std::panic::catch_unwind(check_every_target) {
        Ok(()) => std::process::exit(0),
        Err(_) => std::process::exit(ASSERTION_FAILED_EXIT_CODE),
    }
}

#[test]
fn drop_reports_only_when_the_caller_left_the_drain_to_it() {
    dispatch_if_child();

    let exe = std::env::current_exe().expect("locate the test binary");
    let mut child = Command::new(exe)
        .env(CHILD_VAR, "1")
        .env("WINDOWS_THREADPOOL_TRACE", TRACE_FILTER)
        // The child measures a process-wide trace buffer, so nothing else in it
        // may run concurrently and land records inside the measurement.
        .env("RUST_TEST_THREADS", "1")
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn the child");

    let deadline = std::time::Instant::now() + CHILD_TIMEOUT;
    let status = loop {
        if let Some(status) = child.try_wait().expect("poll the child") {
            break status;
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("the child did not finish within {CHILD_TIMEOUT:?}");
        }
        std::thread::sleep(Duration::from_millis(20));
    };

    assert_ne!(
        status.code(),
        Some(FILTER_MISSING_EXIT_CODE),
        "the child did not receive {TRACE_FILTER:?} through WINDOWS_THREADPOOL_TRACE, \
         so it observed nothing -- the measurement did not run"
    );
    assert!(
        status.success(),
        "the child reported a failure of the obligation contract (exit {:?}); \
         its output is above",
        status.code()
    );
}
