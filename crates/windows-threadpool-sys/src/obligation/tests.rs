// Copyright (c) Mike Grier
//! Unit tests for [`CloseObligation`].
//!
//! These pin the state machine itself. That a given type's `Drop` actually
//! consults it is pinned separately, by each type's own tests and by the
//! end-to-end report assertions in `tests/obligation_report.rs`.

use super::CloseObligation;

#[test]
fn a_fresh_obligation_is_not_owed() {
    // The whole reason the polarity is "owed" rather than "drained": an object
    // that was created and never armed has nothing for `Drop` to wait on, so it
    // must not report.
    assert!(!CloseObligation::new().is_owed());
}

#[test]
fn going_live_owes() {
    let obligation = CloseObligation::new();
    obligation.record_live();
    assert!(obligation.is_owed());
}

#[test]
fn settling_discharges() {
    let obligation = CloseObligation::new();
    obligation.record_live();
    obligation.record_settled();
    assert!(!obligation.is_owed());
}

#[test]
fn going_live_again_after_settling_owes_again() {
    // The sequence a re-armed wait produces: armed, dispatched (which settles),
    // re-armed from inside the callback.
    let obligation = CloseObligation::new();
    obligation.record_live();
    obligation.record_settled();
    obligation.record_live();
    assert!(obligation.is_owed());
}

#[test]
fn settling_an_unowed_obligation_is_harmless() {
    // `stop_and_drain` on an object that was never armed, and a dispatch on a
    // periodic timer that has already been stopped, both reach this.
    let obligation = CloseObligation::new();
    obligation.record_settled();
    obligation.record_settled();
    assert!(!obligation.is_owed());
}

#[test]
fn repeated_arming_owes_once() {
    // `submit` called many times, or a wait armed and re-armed before firing:
    // the flag is a question about whether anything is outstanding, not a count
    // of how much.
    let obligation = CloseObligation::new();
    for _ in 0..16 {
        obligation.record_live();
    }
    assert!(obligation.is_owed());
    obligation.record_settled();
    assert!(!obligation.is_owed());
}

#[test]
fn the_obligation_is_recorded_before_the_arming_is_published() {
    // The ordering every armed object depends on, asserted at the one site that
    // now owns it. `publish` stands for the native call -- `SetThreadpoolWait`,
    // `SetThreadpoolTimer`, `SubmitThreadpoolWork` -- after which a dispatch on
    // a pool thread may settle the obligation at any instant. If the flag were
    // not already set when `publish` runs, that settle would be overwritten by
    // a later store and a drained object would claim it owes a drain.
    let obligation = CloseObligation::new();
    let mut owed_when_published = None;
    obligation.record_live_before(|| owed_when_published = Some(obligation.is_owed()));
    assert_eq!(
        owed_when_published,
        Some(true),
        "the arming was published while the obligation still read as not owed"
    );
    assert!(obligation.is_owed());
}

#[test]
fn a_settle_from_inside_the_publish_survives() {
    // The race itself, made deterministic: `publish` here does what a dispatch
    // racing the arming does -- it settles the obligation before the arming
    // call returns. Nothing may re-assert the obligation afterwards, because
    // the settle is the later event and is the true one.
    //
    // This is the assertion that fails if the store is ever moved back after
    // `publish`, which is how `wait`'s re-arm, `timer`'s deferred re-arm and
    // `work`'s submit were all written before this method existed.
    let obligation = CloseObligation::new();
    obligation.record_live_before(|| obligation.record_settled());
    assert!(
        !obligation.is_owed(),
        "a settle that happened during the arming was overwritten by the record"
    );
}

#[test]
fn the_event_tag_is_stable() {
    // A consumer filters the trace on this string. Changing it is a change to
    // what a reader of a capture has to grep for, so it is pinned here rather
    // than left to drift with a refactor.
    assert_eq!(super::DROP_OBLIGATION_OWED, "drop-obligation-owed");
}

// --- Wiring: that each type's obligation tracks what is actually outstanding.
//
// These do not read the trace. Its filter is fixed by a pre-`main` initialiser,
// so a test cannot narrow it and a trace-reading assertion would silently pass
// in an ordinary `cargo test`. They assert the flag at the exact moment `Drop`
// reads it, which is what decides whether the report is emitted, and they run
// everywhere.

use crate::timer::{ThreadpoolPeriodicTimer, ThreadpoolTimer};
use crate::wait::{ThreadpoolWait, WaitableHandle};
use crate::work::ThreadpoolWork;
use std::os::windows::io::AsRawHandle;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use windows_sys::Win32::System::Threading::SetEvent;

/// Signal the event a wait is watching.
fn signal(wait: &ThreadpoolWait) {
    // SAFETY: the handle is a live event owned by the wait object, which this
    // borrow keeps alive for the call.
    let ok = unsafe { SetEvent(wait.handle().as_raw_handle()) };
    assert_ne!(ok, 0, "SetEvent failed");
}

/// Spin until `done` or a deadline, so a hung expectation fails rather than
/// hanging the suite.
fn wait_for(label: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {label}");
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn a_wait_that_was_never_armed_owes_nothing() {
    let event = WaitableHandle::event(true, false).expect("create event");
    let wait = ThreadpoolWait::new(event, |_| {}, None).expect("create wait");
    assert!(
        !wait.obligation_owed(),
        "a wait that was never armed has nothing for Drop to wait on"
    );
}

#[test]
fn arming_a_wait_owes_a_drain() {
    let event = WaitableHandle::event(true, false).expect("create event");
    let wait = ThreadpoolWait::new(event, |_| {}, None).expect("create wait");
    wait.arm(None);
    assert!(wait.obligation_owed());
    // Assert first, then discharge: the crate obeys the protocol it publishes,
    // which `fail-fast` turns from a convention into a build that proves it.
    wait.stop_and_drain();
}

#[test]
fn stop_and_drain_discharges_a_waits_obligation() {
    let event = WaitableHandle::event(true, false).expect("create event");
    let wait = ThreadpoolWait::new(event, |_| {}, None).expect("create wait");
    wait.arm(None);
    wait.stop_and_drain();
    assert!(
        !wait.obligation_owed(),
        "the caller drained it, so Drop has nothing to report"
    );
}

#[test]
fn a_wait_that_fired_without_rearming_owes_nothing() {
    // The case a "did the caller call the close" flag would get wrong: the
    // caller armed, the callback ran, and the pool is no longer watching, so
    // Drop's drain finds nothing and there is nothing to report.
    let event = WaitableHandle::event(true, false).expect("create event");
    let fired = Arc::new(AtomicUsize::new(0));
    let seen = Arc::clone(&fired);
    let wait = ThreadpoolWait::new(
        event,
        move |_| {
            seen.fetch_add(1, Ordering::SeqCst);
        },
        None,
    )
    .expect("create wait");
    wait.arm(None);
    signal(&wait);
    wait_for("the wait to fire", || fired.load(Ordering::SeqCst) == 1);
    // Asserted here and NOT after a `stop_and_drain`: that call settles the
    // obligation by itself, so asserting after it would pass whether or not the
    // dispatch settled anything, and this test would be measuring nothing. The
    // trampoline clears before invoking the callback, so a non-zero `fired`
    // means the clear has already happened and there is no race.
    assert!(
        !wait.obligation_owed(),
        "the activation consumed the arming, so nothing is outstanding"
    );
}

#[test]
fn a_wait_rearmed_from_its_callback_owes_again() {
    let event = WaitableHandle::event(false, false).expect("create event");
    let fired = Arc::new(AtomicUsize::new(0));
    let seen = Arc::clone(&fired);
    let wait = ThreadpoolWait::new(
        event,
        move |activation| {
            // Re-arm only on the first firing, so the count is deterministic.
            if seen.fetch_add(1, Ordering::SeqCst) == 0 {
                activation.rearm(None);
            }
        },
        None,
    )
    .expect("create wait");
    wait.arm(None);
    signal(&wait);
    wait_for("the wait to fire", || fired.load(Ordering::SeqCst) >= 1);
    // The callback re-armed, so the object is live again and Drop would block.
    wait_for("the re-arm to land", || wait.obligation_owed());
    wait.stop_and_drain();
}

#[test]
fn a_timer_that_was_never_set_owes_nothing() {
    let timer = ThreadpoolTimer::new(|_| {}, None).expect("create timer");
    assert!(!timer.obligation_owed());
}

#[test]
fn setting_a_timer_owes_a_drain() {
    let timer = ThreadpoolTimer::new(|_| {}, None).expect("create timer");
    timer.set_after(Duration::from_secs(60));
    assert!(timer.obligation_owed());
    timer.stop_and_drain();
    assert!(!timer.obligation_owed());
}

#[test]
fn a_one_shot_timer_that_fired_owes_nothing() {
    // `is_set` stays true after a one-shot expires, so it could not answer
    // this. The firing itself is what settles the obligation.
    let fired = Arc::new(AtomicUsize::new(0));
    let seen = Arc::clone(&fired);
    let timer = ThreadpoolTimer::new(
        move |_| {
            seen.fetch_add(1, Ordering::SeqCst);
        },
        None,
    )
    .expect("create timer");
    timer.set_after(Duration::from_millis(1));
    wait_for("the timer to fire", || fired.load(Ordering::SeqCst) == 1);
    // Asserted before any `stop_and_drain`, for the reason given in
    // `a_wait_that_fired_without_rearming_owes_nothing`: draining first would
    // make this pass regardless of what the firing did.
    assert!(
        !timer.obligation_owed(),
        "a one-shot produces one callback per arming, so the firing settled it"
    );
}

#[test]
fn a_periodic_timer_still_owes_after_ticking() {
    // The difference from the one-shot, and the reason a tick must not settle
    // it: the pool re-arms from the period, so the timer is exactly as live
    // after a tick as before one.
    let ticks = Arc::new(AtomicUsize::new(0));
    let seen = Arc::clone(&ticks);
    let timer = ThreadpoolPeriodicTimer::new(
        Duration::from_millis(1),
        move |_| {
            seen.fetch_add(1, Ordering::SeqCst);
        },
        None,
    )
    .expect("create timer");
    assert!(!timer.obligation_owed(), "not started yet");
    timer.start();
    wait_for("three ticks", || ticks.load(Ordering::SeqCst) >= 3);
    assert!(
        timer.obligation_owed(),
        "ticking does not clear the schedule, so the drain is still owed"
    );
    timer.stop_and_drain();
    assert!(!timer.obligation_owed());
}

#[test]
fn work_owes_from_submit_until_wait() {
    let work = ThreadpoolWork::new(|| {}, None).expect("create work");
    assert!(!work.obligation_owed(), "nothing submitted yet");
    work.submit();
    assert!(work.obligation_owed());
    work.wait();
    assert!(
        !work.obligation_owed(),
        "`wait` is this type's drain, so it discharges the obligation"
    );
}

#[test]
fn resubmitting_work_owes_again() {
    let work = ThreadpoolWork::new(|| {}, None).expect("create work");
    work.submit();
    work.wait();
    work.submit();
    assert!(
        work.obligation_owed(),
        "a submission after the drain is a fresh obligation"
    );
    work.wait();
    assert!(!work.obligation_owed());
}
