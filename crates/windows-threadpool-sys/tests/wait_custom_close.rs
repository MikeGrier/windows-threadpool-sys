// Copyright (c) 2026 Mike Grier
//! Integration coverage for wait targets that close with a caller-supplied
//! routine rather than `CloseHandle`.
//!
//! The unit tests prove the mechanism on a single object. These drive it at
//! scale -- hundreds of live waits per teardown path -- because the properties
//! that matter are population-wide: *every* handle closes, each closes *exactly
//! once*, and no handle is closed while its own callback is still executing.
//! Both teardown paths are exercised: an individually-owned `ThreadpoolWait`
//! dropped directly, and a `CleanupGroup` released with and without
//! `cancel_pending`.

#![cfg(windows)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::Duration;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::System::Threading::{CreateEventW, SetEvent};

use windows_threadpool_sys::cleanup_group::CleanupGroup;
use windows_threadpool_sys::wait::{ThreadpoolWait, WaitCloseFn, WaitableHandle};

/// How many waits each scenario builds.
///
/// Large enough that teardown genuinely overlaps executing callbacks and that a
/// single missed or doubled close shows up in the totals, small enough that the
/// whole file stays well inside a normal test run.
const WAITS: usize = 256;

/// How long a callback stays inside the pool, so teardown is really draining.
const CALLBACK_DWELL: Duration = Duration::from_millis(50);

/// Upper bound for waiting on callbacks that really should start.
const START_TIMEOUT: Duration = Duration::from_secs(60);

/// The observations a scenario collects, all reachable from a capture-less
/// `extern "system"` close routine, hence all `'static`.
///
/// Each test owns its own set, so the totals stay correct under `cargo test`,
/// which runs tests as threads in a single process.
struct Probe {
    /// Every handle passed to the close routine, in close order. Its length is
    /// the close count and its uniqueness is the "exactly once" property.
    closed: &'static Mutex<Vec<usize>>,
    /// Callbacks that found their own handle already closed. Must stay zero.
    violations: &'static AtomicUsize,
    /// Callbacks that started, plus the condvar a test blocks on.
    started: &'static (Mutex<usize>, Condvar),
}

impl Probe {
    fn closed_handles(&self) -> Vec<usize> {
        self.closed
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clone()
    }

    fn wait_for_started(&self, target: usize) {
        let (count, condvar) = self.started;
        let count = count.lock().unwrap_or_else(|poison| poison.into_inner());
        let (count, timeout) = condvar
            .wait_timeout_while(count, START_TIMEOUT, |count| *count < target)
            .unwrap_or_else(|poison| poison.into_inner());
        assert!(
            !timeout.timed_out(),
            "timed out waiting for {target} callback(s) to start; saw {count}"
        );
    }

    /// Assert the population-wide properties every scenario must satisfy.
    fn assert_closed_each_exactly_once(&self, expected: usize, path: &str) {
        let mut closed = self.closed_handles();
        assert_eq!(
            closed.len(),
            expected,
            "{path}: every custom target must be closed exactly once"
        );
        closed.sort_unstable();
        closed.dedup();
        assert_eq!(
            closed.len(),
            expected,
            "{path}: a handle was closed more than once"
        );
        assert_eq!(
            self.violations.load(Ordering::SeqCst),
            0,
            "{path}: a handle was closed while its own callback was executing"
        );
    }
}

/// Declare a scenario's `'static` observation state plus the capture-less close
/// routine that records into it, and evaluate to `(Probe, WaitCloseFn)`.
///
/// A close routine is a bare `extern "system"` function pointer, so it cannot
/// capture; the state it records into must therefore be `static`. Declaring
/// that state inside each test -- which this macro does -- keeps the tests
/// independent instead of sharing one global set of counters.
macro_rules! probe {
    () => {{
        static CLOSED: Mutex<Vec<usize>> = Mutex::new(Vec::new());
        static VIOLATIONS: AtomicUsize = AtomicUsize::new(0);
        static STARTED: (Mutex<usize>, Condvar) = (Mutex::new(0), Condvar::new());

        /// Stands in for a routine like `FindCloseChangeNotification`: records
        /// the close, then really releases the handle so nothing leaks.
        unsafe extern "system" fn close(handle: HANDLE) -> i32 {
            CLOSED
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .push(handle as usize);
            // SAFETY: the wait object owned this event and the pool has stopped
            // watching it by the time a close routine runs.
            unsafe { CloseHandle(handle) }
        }

        let probe = Probe {
            closed: &CLOSED,
            violations: &VIOLATIONS,
            started: &STARTED,
        };
        let close: WaitCloseFn = close;
        (probe, close, &CLOSED, &VIOLATIONS, &STARTED)
    }};
}

/// Create a fresh manual-reset event, unsignalled.
fn raw_event() -> HANDLE {
    // SAFETY: an unnamed, manual-reset, initially-unsignalled event with default
    // security attributes; all pointer arguments are null by design.
    let raw = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
    assert!(!raw.is_null(), "CreateEventW failed");
    raw
}

/// Signal a raw event handle.
///
/// # Safety
///
/// `raw` must still be a live event -- that is, the owning wait object must not
/// have been torn down yet.
unsafe fn signal(raw: HANDLE) {
    // SAFETY: forwarded from this function's own contract.
    let ok = unsafe { SetEvent(raw) };
    assert_ne!(ok, 0, "SetEvent failed");
}

/// The callback body shared by every scenario.
///
/// Dwells inside the pool so teardown overlaps it, then checks whether *its own*
/// handle has already been closed. Checking per-handle rather than globally is
/// what makes the ordering assertion meaningful at scale: with hundreds of waits
/// tearing down together, some other handle being closed says nothing, but this
/// one being closed under its own running callback is exactly the bug.
fn observe(
    key: usize,
    closed: &'static Mutex<Vec<usize>>,
    violations: &'static AtomicUsize,
    started: &'static (Mutex<usize>, Condvar),
) {
    {
        let (count, condvar) = started;
        let mut count = count.lock().unwrap_or_else(|poison| poison.into_inner());
        *count += 1;
        condvar.notify_all();
    }
    std::thread::sleep(CALLBACK_DWELL);
    let already_closed = closed
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .contains(&key);
    if already_closed {
        violations.fetch_add(1, Ordering::SeqCst);
    }
}

#[test]
fn dropping_owned_waits_closes_every_custom_target_exactly_once() {
    let (probe, close, closed, violations, started) = probe!();

    let mut waits = Vec::with_capacity(WAITS);
    for _ in 0..WAITS {
        let raw = raw_event();
        let key = raw as usize;
        // SAFETY: a fresh event is a supported wait target, exclusively owned
        // here, and `close` releases exactly the handle it is given.
        let handle = unsafe { WaitableHandle::assume_waitable_with(raw, close) };
        let wait = ThreadpoolWait::new(
            handle,
            move |_| observe(key, closed, violations, started),
            None,
        )
        .expect("create wait");
        wait.arm(None);
        // SAFETY: the wait object owns the event and is armed, so it is live.
        unsafe { signal(raw) };
        waits.push(wait);
    }

    // Tear down while callbacks are genuinely still executing.
    probe.wait_for_started(WAITS / 2);
    assert!(
        probe.closed_handles().is_empty(),
        "nothing may be closed while the waits are alive"
    );
    let entered_teardown = std::time::Instant::now();

    drop(waits);
    let blocked_for = entered_teardown.elapsed();

    assert!(
        blocked_for >= CALLBACK_DWELL / 2,
        "teardown returned in {blocked_for:?}, so it did not drain running callbacks"
    );
    probe.assert_closed_each_exactly_once(WAITS, "direct drop");
}

#[test]
fn releasing_a_group_closes_every_custom_target_exactly_once() {
    let (probe, close, closed, violations, started) = probe!();

    let mut group = CleanupGroup::new().expect("create group");
    let mut raws = Vec::with_capacity(WAITS);
    for _ in 0..WAITS {
        let raw = raw_event();
        let key = raw as usize;
        // SAFETY: as above.
        let handle = unsafe { WaitableHandle::assume_waitable_with(raw, close) };
        let member = group
            .create_wait(
                handle,
                move |_| observe(key, closed, violations, started),
                None,
            )
            .expect("create wait");
        member.arm(None);
        // SAFETY: the group owns the event and the member is armed.
        unsafe { signal(raw) };
        raws.push(raw);
    }

    probe.wait_for_started(WAITS / 2);
    assert!(
        probe.closed_handles().is_empty(),
        "nothing may be closed while the members are live"
    );
    assert_eq!(
        group.owned_resources(),
        WAITS * 2,
        "each member parks a context and a target on the group"
    );
    let entered_teardown = std::time::Instant::now();

    group.close_members(false);
    let blocked_for = entered_teardown.elapsed();

    assert!(
        blocked_for >= CALLBACK_DWELL / 2,
        "the release returned in {blocked_for:?}, so it did not drain running callbacks"
    );
    probe.assert_closed_each_exactly_once(WAITS, "group release");
    assert_eq!(group.owned_resources(), 0, "the group holds nothing after");

    // A second release, and the group's own drop, must not close anything again.
    group.close_members(false);
    drop(group);
    probe.assert_closed_each_exactly_once(WAITS, "group release, repeated");
}

#[test]
fn releasing_a_group_with_cancel_pending_closes_every_custom_target_exactly_once() {
    let (probe, close, closed, violations, started) = probe!();

    let mut group = CleanupGroup::new().expect("create group");
    for _ in 0..WAITS {
        let raw = raw_event();
        let key = raw as usize;
        // SAFETY: as above.
        let handle = unsafe { WaitableHandle::assume_waitable_with(raw, close) };
        let member = group
            .create_wait(
                handle,
                move |_| observe(key, closed, violations, started),
                None,
            )
            .expect("create wait");
        member.arm(None);
        // SAFETY: the group owns the event and the member is armed.
        unsafe { signal(raw) };
    }

    // Cancelling drops callbacks that have not started, so only wait for one --
    // enough that the release genuinely overlaps an executing callback, while
    // most are still queued and will be discarded.
    probe.wait_for_started(1);
    assert!(
        probe.closed_handles().is_empty(),
        "nothing may be closed while the members are live"
    );

    group.close_members(true);

    // Whether a callback ran, was cancelled, or was mid-flight, the handle is
    // still the group's to close, exactly once, with the caller's routine.
    probe.assert_closed_each_exactly_once(WAITS, "group release, cancel_pending");
}

#[test]
fn a_group_releases_custom_and_default_targets_together() {
    // The custom-close seam must leave the `OwnedHandle` default alone even when
    // both kinds of member are torn down by the same release.
    let (probe, close, closed, violations, started) = probe!();

    let mut group = CleanupGroup::new().expect("create group");
    for index in 0..WAITS {
        if index % 2 == 0 {
            let raw = raw_event();
            let key = raw as usize;
            // SAFETY: as above.
            let handle = unsafe { WaitableHandle::assume_waitable_with(raw, close) };
            let member = group
                .create_wait(
                    handle,
                    move |_| observe(key, closed, violations, started),
                    None,
                )
                .expect("create custom wait");
            member.arm(None);
            // SAFETY: the group owns the event and the member is armed.
            unsafe { signal(raw) };
        } else {
            let handle = WaitableHandle::event(true, false).expect("create an event");
            let member = group
                .create_wait(handle, |_| std::thread::sleep(CALLBACK_DWELL), None)
                .expect("create owned wait");
            member.arm(None);
            // The default path's handle is closed by `OwnedHandle`, which the
            // probe cannot see -- its absence from the close log is the point.
        }
    }

    group.close_members(false);

    probe.assert_closed_each_exactly_once(WAITS / 2, "mixed group release");
    assert_eq!(group.owned_resources(), 0, "the group holds nothing after");
}
