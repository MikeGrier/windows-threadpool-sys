// Copyright (c) 2026 Mike Grier
//! Unit tests for thread-pool waits.
//!
//! Every test drives a real event handle, since the wait object's contract is
//! about handle ownership and per-activation rearming rather than about timing.

use std::os::windows::io::AsRawHandle;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use windows_sys::Win32::System::Threading::{ResetEvent, SetEvent};

use crate::callback_env::CallbackEnviron;
use crate::pool::ThreadpoolPool;
use crate::wait::{ThreadpoolWait, WaitResult, WaitableHandle};

/// Upper bound for waiting on an activation the object really should deliver.
const ACTIVATION_TIMEOUT: Duration = Duration::from_secs(30);

/// Create an auto-reset or manual-reset event, initially unsignalled.
fn event(manual_reset: bool) -> WaitableHandle {
    WaitableHandle::event(manual_reset, false).expect("create an event")
}

fn signal(handle: std::os::windows::io::BorrowedHandle<'_>) {
    // SAFETY: the handle is a live event owned by the wait object.
    let ok = unsafe { SetEvent(handle.as_raw_handle()) };
    assert_ne!(
        ok,
        0,
        "SetEvent failed: {}",
        std::io::Error::last_os_error()
    );
}

fn reset(handle: std::os::windows::io::BorrowedHandle<'_>) {
    // SAFETY: the handle is a live event owned by the wait object.
    let ok = unsafe { ResetEvent(handle.as_raw_handle()) };
    assert_ne!(
        ok,
        0,
        "ResetEvent failed: {}",
        std::io::Error::last_os_error()
    );
}

/// Records activations and lets a test block until a target count is reached.
struct Activations {
    seen: Mutex<Vec<WaitResult>>,
    arrived: Condvar,
}

impl Activations {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            seen: Mutex::new(Vec::new()),
            arrived: Condvar::new(),
        })
    }

    fn record(&self, result: WaitResult) {
        let mut seen = self.seen.lock().expect("record an activation");
        seen.push(result);
        self.arrived.notify_all();
    }

    fn count(&self) -> usize {
        self.seen.lock().expect("read activations").len()
    }

    fn results(&self) -> Vec<WaitResult> {
        self.seen.lock().expect("read activations").clone()
    }

    fn wait_for(&self, target: usize) -> Vec<WaitResult> {
        let seen = self.seen.lock().expect("await activations");
        let (seen, timeout) = self
            .arrived
            .wait_timeout_while(seen, ACTIVATION_TIMEOUT, |seen| seen.len() < target)
            .expect("await activations");
        assert!(
            !timeout.timed_out(),
            "timed out waiting for {target} activation(s); saw {}",
            seen.len()
        );
        seen.clone()
    }
}

/// A wait that records each activation without rearming.
fn recording_wait(manual_reset: bool) -> (ThreadpoolWait, Arc<Activations>) {
    let seen = Activations::new();
    let recorder = Arc::clone(&seen);
    let wait = ThreadpoolWait::new(
        event(manual_reset),
        move |activation| recorder.record(activation.result()),
        None,
    )
    .expect("create wait");
    (wait, seen)
}

// --- creation ---

#[test]
fn new_wait_succeeds() {
    assert!(ThreadpoolWait::new(event(true), |_| {}, None).is_ok());
}

#[test]
fn new_wait_with_env_succeeds() {
    let mut env = CallbackEnviron::new();
    assert!(ThreadpoolWait::new(event(true), |_| {}, Some(&mut env)).is_ok());
}

#[test]
fn drop_without_arming_is_clean() {
    let _wait = ThreadpoolWait::new(event(true), |_| {}, None).expect("create wait");
}

#[test]
fn the_wait_exposes_its_handle() {
    let (wait, _seen) = recording_wait(true);
    // Signalling through the borrowed handle must succeed, proving the object
    // kept it open.
    signal(wait.handle());
}

// --- signalled activation ---

#[test]
fn signalling_activates_the_callback() {
    let (wait, seen) = recording_wait(true);
    wait.arm(None);
    signal(wait.handle());
    let results = seen.wait_for(1);
    wait.wait();
    assert_eq!(results, vec![WaitResult::Signalled]);
}

#[test]
fn an_already_signalled_handle_activates_on_arming() {
    let (wait, seen) = recording_wait(true);
    // Signal before arming; the wait must notice the existing signal.
    signal(wait.handle());
    wait.arm(None);
    let results = seen.wait_for(1);
    wait.wait();
    assert_eq!(results, vec![WaitResult::Signalled]);
}

#[test]
fn an_activation_reports_signalled() {
    let (wait, seen) = recording_wait(true);
    wait.arm(None);
    signal(wait.handle());
    let results = seen.wait_for(1);
    wait.wait();
    assert!(results[0] == WaitResult::Signalled);
}

/// One arming yields exactly one activation, even if the handle stays signalled.
#[test]
fn one_arming_yields_one_activation() {
    let (wait, seen) = recording_wait(true);
    wait.arm(None);
    signal(wait.handle());
    seen.wait_for(1);
    wait.wait();

    // The manual-reset event is still signalled, but the wait was consumed.
    std::thread::sleep(Duration::from_millis(60));
    assert_eq!(
        seen.count(),
        1,
        "an activation consumes the arming; it must not repeat"
    );
}

// --- timeout activation ---

#[test]
fn a_timeout_activates_the_callback() {
    let (wait, seen) = recording_wait(true);
    wait.arm(Some(Duration::from_millis(20)));
    let results = seen.wait_for(1);
    wait.wait();
    assert_eq!(results, vec![WaitResult::TimedOut]);
}

#[test]
fn signalling_before_the_timeout_reports_signalled() {
    let (wait, seen) = recording_wait(true);
    wait.arm(Some(Duration::from_secs(30)));
    signal(wait.handle());
    let results = seen.wait_for(1);
    wait.wait();
    assert_eq!(results, vec![WaitResult::Signalled]);
}

#[test]
fn a_zero_timeout_activates_promptly() {
    let (wait, seen) = recording_wait(true);
    wait.arm(Some(Duration::ZERO));
    let results = seen.wait_for(1);
    wait.wait();
    assert_eq!(results, vec![WaitResult::TimedOut]);
}

// --- rearming ---

/// The SDK requires explicit rearming per activation, so a callback that rearms
/// keeps watching across several signals.
#[test]
fn a_rearming_callback_activates_repeatedly() {
    const SIGNALS: usize = 10;

    let seen = Activations::new();
    let recorder = Arc::clone(&seen);
    let wait = ThreadpoolWait::new(
        event(false),
        move |activation| {
            recorder.record(activation.result());
            activation.rearm(None);
        },
        None,
    )
    .expect("create wait");

    wait.arm(None);
    for expected in 1..=SIGNALS {
        signal(wait.handle());
        seen.wait_for(expected);
    }

    wait.disarm();
    wait.wait();
    let results = seen.results();
    assert!(results.len() >= SIGNALS);
    assert!(
        results.iter().all(|r| *r == WaitResult::Signalled),
        "every activation should be a signal, got {results:?}"
    );
}

/// A callback that does not rearm stops watching after one activation.
#[test]
fn a_callback_that_does_not_rearm_stops_watching() {
    let (wait, seen) = recording_wait(false);
    wait.arm(None);
    signal(wait.handle());
    seen.wait_for(1);
    wait.wait();

    // A second signal must not activate anything, since nothing rearmed.
    signal(wait.handle());
    std::thread::sleep(Duration::from_millis(60));
    assert_eq!(seen.count(), 1);
}

#[test]
fn rearming_with_a_timeout_activates_on_timeout() {
    let seen = Activations::new();
    let recorder = Arc::clone(&seen);
    let rearmed = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&rearmed);

    let wait = ThreadpoolWait::new(
        event(false),
        move |activation| {
            recorder.record(activation.result());
            // Rearm a bounded number of times so the test terminates.
            if counter.fetch_add(1, Ordering::SeqCst) < 2 {
                activation.rearm(Some(Duration::from_millis(10)));
            }
        },
        None,
    )
    .expect("create wait");

    wait.arm(Some(Duration::from_millis(10)));
    let results = seen.wait_for(3);
    wait.disarm();
    wait.wait();
    assert!(
        results.iter().all(|r| *r == WaitResult::TimedOut),
        "expected timeouts, got {results:?}"
    );
}

#[test]
fn arming_again_from_outside_replaces_the_previous_arming() {
    let (wait, seen) = recording_wait(true);
    wait.arm(Some(Duration::from_secs(30)));
    // Replace the long timeout with a short one.
    wait.arm(Some(Duration::from_millis(20)));
    let results = seen.wait_for(1);
    wait.wait();
    assert_eq!(results, vec![WaitResult::TimedOut]);
}

// --- disarming ---

#[test]
fn disarming_prevents_activation() {
    let (wait, seen) = recording_wait(true);
    wait.arm(None);
    wait.disarm();
    signal(wait.handle());
    std::thread::sleep(Duration::from_millis(60));
    assert_eq!(seen.count(), 0, "a disarmed wait must not activate");
}

#[test]
fn disarming_an_idle_wait_is_a_no_op() {
    let (wait, seen) = recording_wait(true);
    wait.disarm();
    wait.disarm();
    assert_eq!(seen.count(), 0);
}

#[test]
fn a_wait_can_be_rearmed_after_disarming() {
    let (wait, seen) = recording_wait(true);
    wait.arm(None);
    wait.disarm();
    reset(wait.handle());

    wait.arm(None);
    signal(wait.handle());
    let results = seen.wait_for(1);
    wait.wait();
    assert_eq!(results, vec![WaitResult::Signalled]);
}

// --- quiescing without dropping ---

/// Build a wait whose callback always re-arms, on a manual-reset event that
/// stays signalled so activations keep coming.
fn always_rearming_wait() -> (ThreadpoolWait, Arc<Activations>) {
    let seen = Activations::new();
    let recorder = Arc::clone(&seen);
    let wait = ThreadpoolWait::new(
        WaitableHandle::event(true, true).expect("create a signalled event"),
        move |activation| {
            recorder.record(activation.result());
            std::thread::sleep(Duration::from_millis(60));
            activation.rearm(None);
        },
        None,
    )
    .expect("create wait");
    (wait, seen)
}

/// Wait until a callback has been entered, so the caller acts while one runs.
fn wait_until_activated(seen: &Activations) {
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while seen.count() == 0 {
        assert!(
            std::time::Instant::now() < deadline,
            "the wait never activated"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// `stop_and_drain` must leave a self-re-arming wait idle, which is what
/// distinguishes it from `disarm` plus a drain.
#[test]
fn stop_and_drain_quiesces_a_self_rearming_wait() {
    let (wait, seen) = always_rearming_wait();

    wait.arm(None);
    wait_until_activated(&seen);
    wait.stop_and_drain();

    let settled = seen.count();
    std::thread::sleep(Duration::from_millis(150));
    assert_eq!(
        seen.count(),
        settled,
        "the wait kept activating after stop_and_drain"
    );
}

/// The suppression must be lifted on return, or the wait would be permanently
/// dead rather than merely idle.
#[test]
fn a_wait_is_reusable_after_stop_and_drain() {
    let (wait, seen) = always_rearming_wait();

    wait.arm(None);
    wait_until_activated(&seen);
    wait.stop_and_drain();
    let settled = seen.count();

    wait.arm(None);
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while seen.count() == settled {
        assert!(
            std::time::Instant::now() < deadline,
            "the wait never activated again after stop_and_drain"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    wait.stop_and_drain();
}

/// Concurrent callers must not lift one another's suppression, which is why the
/// suppression is a count rather than a flag.
#[test]
fn concurrent_stop_and_drain_calls_all_quiesce_a_wait() {
    let (wait, seen) = always_rearming_wait();
    let wait = Arc::new(wait);

    wait.arm(None);
    wait_until_activated(&seen);

    let callers: Vec<_> = (0..4)
        .map(|_| {
            let wait = Arc::clone(&wait);
            std::thread::spawn(move || wait.stop_and_drain())
        })
        .collect();
    for caller in callers {
        caller.join().expect("stop_and_drain thread");
    }

    let settled = seen.count();
    std::thread::sleep(Duration::from_millis(150));
    assert_eq!(
        seen.count(),
        settled,
        "the wait kept activating after concurrent stop_and_drain calls"
    );
}

#[test]
fn stop_and_drain_on_an_idle_wait_is_a_no_op() {
    let (wait, seen) = recording_wait(true);
    wait.stop_and_drain();
    assert_eq!(seen.count(), 0);
    // Still usable afterwards.
    wait.arm(None);
    signal(wait.handle());
    seen.wait_for(1);
    wait.stop_and_drain();
}

// --- waitable handle provenance ---

#[test]
fn a_manual_reset_event_is_a_valid_wait_target() {
    assert!(WaitableHandle::event(true, false).is_ok());
}

#[test]
fn an_auto_reset_event_is_a_valid_wait_target() {
    assert!(WaitableHandle::event(false, false).is_ok());
}

/// An initially-signalled event activates as soon as the wait is armed.
#[test]
fn an_initially_signalled_event_activates_on_arming() {
    let seen = Activations::new();
    let recorder = Arc::clone(&seen);
    let handle = WaitableHandle::event(true, true).expect("create a signalled event");
    let wait = ThreadpoolWait::new(
        handle,
        move |activation| recorder.record(activation.result()),
        None,
    )
    .expect("create wait");

    wait.arm(None);
    let results = seen.wait_for(1);
    wait.wait();
    assert_eq!(results, vec![WaitResult::Signalled]);
}

// --- destruction ---

/// Drop must wait for an executing callback before freeing the context.
#[test]
fn drop_waits_for_an_executing_callback() {
    let done = Arc::new(AtomicUsize::new(0));
    let flag = Arc::clone(&done);
    let entered = Activations::new();
    let started = Arc::clone(&entered);

    let handle = event(true);
    // Duplicate-free signalling: keep a raw copy to signal after the object is
    // built, through the object's own borrow.
    {
        let wait = ThreadpoolWait::new(
            handle,
            move |activation| {
                started.record(activation.result());
                std::thread::sleep(Duration::from_millis(30));
                flag.fetch_add(1, Ordering::SeqCst);
            },
            None,
        )
        .expect("create wait");

        wait.arm(None);
        signal(wait.handle());
        // Only return once the callback is genuinely running.
        entered.wait_for(1);
        // Drop here.
    }

    assert_eq!(
        done.load(Ordering::SeqCst),
        1,
        "Drop returned while a callback was still executing"
    );
}

/// Dropping a wait whose callback rearms must terminate: Drop disarms first.
#[test]
fn drop_of_a_rearming_wait_terminates() {
    let started = std::time::Instant::now();
    {
        let seen = Activations::new();
        let recorder = Arc::clone(&seen);
        let wait = ThreadpoolWait::new(
            event(false),
            move |activation| {
                recorder.record(activation.result());
                // Immediately rearm with a short timeout, so the object keeps
                // queueing activations until it is disarmed.
                activation.rearm(Some(Duration::from_millis(1)));
            },
            None,
        )
        .expect("create wait");

        wait.arm(Some(Duration::from_millis(1)));
        seen.wait_for(3);
        // Drop here, with the wait still rearming itself.
    }
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "dropping a self-rearming wait appears to have hung"
    );
}

/// Outside of teardown, a re-arm request is honoured. This is the control for
/// [`rearming_during_teardown_is_suppressed`], which asserts the opposite.
#[test]
fn rearming_outside_teardown_is_honoured() {
    let outcomes = Arc::new(Mutex::new(Vec::new()));
    let recorder = Arc::clone(&outcomes);
    let started = Activations::new();
    let entered = Arc::clone(&started);

    let wait = ThreadpoolWait::new(
        event(true),
        move |activation| {
            // Re-arm only on the first activation, so this terminates.
            if entered.count() == 0 {
                let armed = activation.rearm_reporting(None);
                recorder.lock().unwrap().push(armed);
            }
            entered.record(activation.result());
        },
        None,
    )
    .expect("create wait");

    wait.arm(None);
    signal(wait.handle());
    started.wait_for(2);
    wait.disarm();
    wait.wait();

    assert_eq!(*outcomes.lock().unwrap(), vec![true]);
}

/// A callback that re-arms while `Drop` is tearing down must not leave the
/// object armed behind it: the drain would then return with an activation still
/// possible, and the close and context free would race a fresh callback.
///
/// The callback sleeps long enough for the dropping thread to set the teardown
/// flag, so the re-arm request lands during teardown rather than before it, and
/// asserts on [`WaitActivation::rearm_reporting`] rather than on the absence of
/// undefined behaviour -- the latter is not observable, so a test written that
/// way would pass with the suppression removed.
#[test]
fn rearming_during_teardown_is_suppressed() {
    let outcomes = Arc::new(Mutex::new(Vec::new()));
    let recorder = Arc::clone(&outcomes);
    let started = Activations::new();
    let entered = Arc::clone(&started);

    let elapsed = std::time::Instant::now();
    {
        let wait = ThreadpoolWait::new(
            event(true),
            move |activation| {
                entered.record(activation.result());
                // Give the dropping thread time to flag teardown.
                std::thread::sleep(Duration::from_millis(200));
                let armed = activation.rearm_reporting(None);
                recorder.lock().unwrap().push(armed);
            },
            None,
        )
        .expect("create wait");

        wait.arm(None);
        signal(wait.handle());
        started.wait_for(1);
        // Drop here, concurrently with the callback's re-arm.
    }

    assert!(
        elapsed.elapsed() < Duration::from_secs(10),
        "teardown appears to have hung"
    );
    assert_eq!(
        *outcomes.lock().unwrap(),
        vec![false],
        "the re-arm should have been suppressed by teardown"
    );
}

#[test]
fn drop_while_armed_but_not_signalled_is_clean() {
    let (wait, seen) = recording_wait(true);
    wait.arm(None);
    drop(wait);
    assert_eq!(seen.count(), 0);
}

// --- callback state, environment, and thread-safety ---

#[test]
fn the_callback_may_own_heap_state() {
    let data = Arc::new(vec![10_u64, 20, 30]);
    let sum = Arc::new(AtomicUsize::new(0));
    let total = Arc::clone(&sum);
    let seen = Activations::new();
    let recorder = Arc::clone(&seen);

    let wait = ThreadpoolWait::new(
        event(true),
        move |activation| {
            total.fetch_add(data.iter().sum::<u64>() as usize, Ordering::SeqCst);
            recorder.record(activation.result());
        },
        None,
    )
    .expect("create wait");

    wait.arm(None);
    signal(wait.handle());
    seen.wait_for(1);
    wait.wait();
    assert_eq!(sum.load(Ordering::SeqCst), 60);
}

#[test]
fn a_wait_runs_on_a_private_pool() {
    // Declared before the wait so it outlives it.
    let pool = ThreadpoolPool::new().expect("create pool");
    let mut env = CallbackEnviron::new();
    env.set_pool(&pool);

    let seen = Activations::new();
    let recorder = Arc::clone(&seen);
    let wait = ThreadpoolWait::new(
        event(true),
        move |activation| recorder.record(activation.result()),
        Some(&mut env),
    )
    .expect("create wait");

    wait.arm(None);
    signal(wait.handle());
    let results = seen.wait_for(1);
    wait.wait();
    assert_eq!(results, vec![WaitResult::Signalled]);
}

/// A panicking callback must be contained at the FFI boundary.
///
/// The caught panic prints to stderr; that output is expected.
#[test]
fn a_panicking_callback_is_contained() {
    let seen = Activations::new();
    let recorder = Arc::clone(&seen);
    let wait = ThreadpoolWait::new(
        event(true),
        move |activation| {
            recorder.record(activation.result());
            panic!("wait callback panics on purpose");
        },
        None,
    )
    .expect("create wait");

    wait.arm(None);
    signal(wait.handle());
    seen.wait_for(1);
    wait.wait();
    assert_eq!(seen.count(), 1);
}

#[test]
fn a_wait_is_send_and_sync() {
    fn assert_send<T: Send>() {}
    fn assert_sync<T: Sync>() {}
    assert_send::<ThreadpoolWait>();
    assert_sync::<ThreadpoolWait>();
}

#[test]
fn a_wait_can_be_armed_from_another_thread() {
    let (wait, seen) = recording_wait(true);
    std::thread::scope(|scope| {
        let wait = &wait;
        scope.spawn(move || {
            wait.arm(None);
            signal(wait.handle());
        });
    });
    let results = seen.wait_for(1);
    wait.wait();
    assert_eq!(results, vec![WaitResult::Signalled]);
}

// --- result mapping ---

#[test]
fn wait_result_maps_the_documented_values() {
    let (wait, seen) = recording_wait(true);
    wait.arm(Some(Duration::from_millis(10)));
    let timed_out = seen.wait_for(1);
    wait.wait();
    assert_eq!(timed_out[0], WaitResult::TimedOut);
    assert!(!matches!(timed_out[0], WaitResult::Other(_)));

    reset(wait.handle());
    wait.arm(None);
    signal(wait.handle());
    let signalled = seen.wait_for(2);
    wait.wait();
    assert_eq!(signalled[1], WaitResult::Signalled);
}

#[test]
fn is_signalled_agrees_with_the_result() {
    let flags = Arc::new(Mutex::new(Vec::new()));
    let recorder = Arc::clone(&flags);
    let seen = Activations::new();
    let counter = Arc::clone(&seen);

    let wait = ThreadpoolWait::new(
        event(true),
        move |activation| {
            recorder
                .lock()
                .expect("record")
                .push((activation.is_signalled(), activation.result()));
            counter.record(activation.result());
        },
        None,
    )
    .expect("create wait");

    wait.arm(Some(Duration::from_millis(10)));
    seen.wait_for(1);
    wait.wait();

    let recorded = flags.lock().expect("read").clone();
    assert_eq!(recorded[0], (false, WaitResult::TimedOut));
}
