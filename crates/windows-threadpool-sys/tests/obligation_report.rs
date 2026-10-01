// Copyright (c) Mike Grier
//! That `Drop` actually emits the undischarged-obligation report, end to end.
//!
//! This is a separate binary with a single test for two reasons. The trace's
//! filter is fixed by a pre-`main` initialiser, so it can only be narrowed by
//! launching the process with `WINDOWS_THREADPOOL_TRACE` set -- a test cannot
//! set it for itself. And `dump` returns the whole buffer, so a sibling test
//! dropping its own pool object concurrently would land records in the middle
//! of this one's measurement.
//!
//! Run it with the trace narrowed to the subsystems below:
//!
//! ```text
//! $env:WINDOWS_THREADPOOL_TRACE = "wait,timer,timer-periodic,work"
//! cargo test -p windows-threadpool-sys --features trace --test obligation_report
//! ```
//!
//! Without that, the body cannot observe anything and the test says so rather
//! than passing quietly -- a check that reports success while measuring nothing
//! is worse than one that is absent.

#![cfg(all(windows, feature = "trace"))]

use std::time::Duration;
use windows_threadpool_sys::timer::{ThreadpoolPeriodicTimer, ThreadpoolTimer};
use windows_threadpool_sys::wait::{ThreadpoolWait, WaitableHandle};
use windows_threadpool_sys::work::ThreadpoolWork;

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

#[test]
fn drop_reports_only_when_the_caller_left_the_drain_to_it() {
    let targets = ["wait", "timer", "timer-periodic", "work"];
    let traced: Vec<&str> = targets
        .into_iter()
        .filter(|t| windows_threadpool_sys::trace::wants(t))
        .collect();
    if traced.is_empty() {
        // Not a failure: the trace feature is on but the environment did not
        // narrow it, which is the ordinary `cargo test --features trace` case
        // and not something this test should block. It is announced rather than
        // passed over in silence, because a check that reports success while
        // observing nothing is the failure mode this file exists to avoid.
        eprintln!(
            "SKIPPED: the trace is armed for none of {targets:?}, so this test \
             can observe nothing. Set WINDOWS_THREADPOOL_TRACE before launching \
             to make it meaningful -- see this file's module docs."
        );
        return;
    }

    for target in traced {
        let before = counted(target, TAG);

        // Discharged: the caller drained it, so Drop has nothing to report.
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
        assert_eq!(
            counted(target, TAG),
            before,
            "`{target}` reported an obligation the caller had already discharged"
        );

        // Undischarged: the caller armed it and dropped it, so Drop does the
        // drain and says so.
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
        assert_eq!(
            counted(target, TAG),
            before + 1,
            "`{target}` did not report an obligation the caller left to Drop"
        );
    }
}
