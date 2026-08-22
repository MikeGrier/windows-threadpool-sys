// Copyright (c) 2026 Mike Grier
//! Unit tests for one-shot thread-pool timers.
//!
//! Timing tests use generous upper bounds and assert on observable ordering
//! rather than on precise expiry, because the thread pool coalesces timers and a
//! loaded machine can delay any callback. What is asserted exactly is the
//! contract: how many times a callback ran, and whether the object was armed.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant, SystemTime};

use crate::callback_env::CallbackEnviron;
use crate::pool::ThreadpoolPool;
use crate::timer::ThreadpoolTimer;

/// Upper bound for waiting on a callback the timer really should deliver.
const FIRE_TIMEOUT: Duration = Duration::from_secs(30);

/// Counts callbacks and lets a test block until a target count is reached.
pub(crate) struct Fires {
    count: Mutex<usize>,
    fired: Condvar,
}

impl Fires {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            count: Mutex::new(0),
            fired: Condvar::new(),
        })
    }

    pub(crate) fn record(&self) {
        let mut count = self.count.lock().expect("record a fire");
        *count += 1;
        self.fired.notify_all();
    }

    pub(crate) fn count(&self) -> usize {
        *self.count.lock().expect("read the count")
    }

    /// Block until at least `target` callbacks have run, failing rather than
    /// hanging if they never do.
    pub(crate) fn wait_for(&self, target: usize) {
        let count = self.count.lock().expect("await fires");
        let (count, timeout) = self
            .fired
            .wait_timeout_while(count, FIRE_TIMEOUT, |count| *count < target)
            .expect("await fires");
        assert!(
            !timeout.timed_out(),
            "timed out waiting for {target} callback(s); saw {count}"
        );
    }
}

fn counting_timer() -> (ThreadpoolTimer, Arc<Fires>) {
    let fires = Fires::new();
    let recorder = Arc::clone(&fires);
    let timer = ThreadpoolTimer::new(move |_firing| recorder.record(), None).expect("create timer");
    (timer, fires)
}

// --- creation ---

#[test]
fn new_timer_succeeds() {
    assert!(ThreadpoolTimer::new(|_| {}, None).is_ok());
}

#[test]
fn new_timer_with_env_succeeds() {
    let mut env = CallbackEnviron::new();
    assert!(ThreadpoolTimer::new(|_| {}, Some(&mut env)).is_ok());
}

#[test]
fn new_timer_is_idle() {
    let timer = ThreadpoolTimer::new(|_| {}, None).expect("create timer");
    assert!(!timer.is_set(), "a fresh timer must not be armed");
}

#[test]
fn drop_without_arming_is_clean() {
    let _timer = ThreadpoolTimer::new(|_| {}, None).expect("create timer");
}

// --- one-shot firing ---

#[test]
fn set_after_fires_once() {
    let (timer, fires) = counting_timer();
    timer.set_after(Duration::from_millis(1));
    fires.wait_for(1);
    timer.wait();
    assert_eq!(fires.count(), 1, "a one-shot timer must fire exactly once");
}

/// The defining property of this type: one arming yields one firing, and no
/// more, even after waiting well past any plausible period.
#[test]
fn one_arming_never_fires_twice() {
    let (timer, fires) = counting_timer();
    timer.set_after(Duration::from_millis(1));
    fires.wait_for(1);
    timer.wait();

    std::thread::sleep(Duration::from_millis(80));
    assert_eq!(fires.count(), 1, "a one-shot timer must not repeat");
}

#[test]
fn set_after_zero_fires_immediately() {
    let (timer, fires) = counting_timer();
    timer.set_after(Duration::ZERO);
    fires.wait_for(1);
    timer.wait();
    assert_eq!(fires.count(), 1);
}

/// Expiring does not clear a timer's due time; only disarming does. This pins
/// the contract of `is_set`, which reports "armed and not disarmed" rather than
/// "will fire again".
#[test]
fn a_fired_timer_stays_set_until_disarmed() {
    let (timer, fires) = counting_timer();
    timer.set_after(Duration::from_millis(1));
    fires.wait_for(1);
    timer.wait();
    assert!(
        timer.is_set(),
        "expiry alone must not clear the due time -- is_set reports armed-ness, not pendency"
    );
    timer.disarm();
    assert!(!timer.is_set(), "disarm must clear the due time");
}

#[test]
fn set_after_respects_the_delay() {
    let (timer, fires) = counting_timer();
    let started = Instant::now();
    timer.set_after(Duration::from_millis(120));
    fires.wait_for(1);
    let elapsed = started.elapsed();
    timer.wait();
    // The pool may fire late but must never fire appreciably early. A small
    // tolerance absorbs timer-resolution rounding.
    assert!(
        elapsed >= Duration::from_millis(90),
        "fired after only {elapsed:?}, well before the 120ms due time"
    );
}

#[test]
fn rearming_replaces_the_previous_due_time() {
    let (timer, fires) = counting_timer();
    timer.set_after(Duration::from_secs(3_600));
    timer.set_after(Duration::from_millis(1));
    fires.wait_for(1);
    timer.wait();
    assert_eq!(fires.count(), 1, "rearming must replace, not accumulate");
}

#[test]
fn a_timer_can_be_rearmed_after_firing() {
    let (timer, fires) = counting_timer();
    for expected in 1..=5 {
        timer.set_after(Duration::from_millis(1));
        fires.wait_for(expected);
        timer.wait();
        assert_eq!(fires.count(), expected);
    }
}

// --- re-arming from inside the callback ---

/// Re-arming from inside the callback is how a caller gets repetition that can
/// never overlap itself.
#[test]
fn a_callback_can_rearm_itself() {
    const RUNS: usize = 5;

    let fires = Fires::new();
    let recorder = Arc::clone(&fires);
    let timer = ThreadpoolTimer::new(
        move |firing| {
            let seen = recorder.count();
            recorder.record();
            if seen + 1 < RUNS {
                firing.rearm_after(Duration::from_millis(1));
            }
        },
        None,
    )
    .expect("create timer");

    timer.set_after(Duration::from_millis(1));
    fires.wait_for(RUNS);
    timer.disarm();
    timer.wait();
    assert_eq!(fires.count(), RUNS, "self-rearming must run exactly {RUNS}");
}

/// A self-rearming one-shot never overlaps itself, which is the whole reason to
/// prefer it over a periodic timer for slow callbacks.
#[test]
fn a_self_rearming_callback_never_overlaps() {
    let concurrent = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let fires = Fires::new();

    let in_flight = Arc::clone(&concurrent);
    let high_water = Arc::clone(&peak);
    let recorder = Arc::clone(&fires);

    let timer = ThreadpoolTimer::new(
        move |firing| {
            let now = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
            high_water.fetch_max(now, Ordering::SeqCst);
            // Hold the callback far longer than the re-arm delay.
            std::thread::sleep(Duration::from_millis(15));
            in_flight.fetch_sub(1, Ordering::SeqCst);
            recorder.record();
            firing.rearm_after(Duration::from_millis(1));
        },
        None,
    )
    .expect("create timer");

    timer.set_after(Duration::from_millis(1));
    fires.wait_for(4);
    timer.disarm();
    timer.wait();

    assert_eq!(
        peak.load(Ordering::SeqCst),
        1,
        "a self-rearming one-shot must never run concurrently with itself"
    );
}

/// Re-arming at the *start* of a slow callback must still not overlap, because
/// the request is applied only after the callback returns. Arming immediately
/// would let the next firing begin while this one is still running.
#[test]
fn rearming_early_in_a_slow_callback_still_does_not_overlap() {
    let concurrent = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let fires = Fires::new();

    let in_flight = Arc::clone(&concurrent);
    let high_water = Arc::clone(&peak);
    let recorder = Arc::clone(&fires);

    let timer = ThreadpoolTimer::new(
        move |firing| {
            // Requested first, with a delay far shorter than this callback runs.
            firing.rearm_after(Duration::from_millis(1));
            let now = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
            high_water.fetch_max(now, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(15));
            in_flight.fetch_sub(1, Ordering::SeqCst);
            recorder.record();
        },
        None,
    )
    .expect("create timer");

    timer.set_after(Duration::from_millis(1));
    fires.wait_for(4);
    timer.disarm();
    timer.wait();

    assert_eq!(
        peak.load(Ordering::SeqCst),
        1,
        "re-arming early must not let the next firing start before this one ends"
    );
}

/// The last request in a firing wins, rather than each one arming separately.
#[test]
fn the_last_rearm_request_in_a_firing_wins() {
    let fires = Fires::new();
    let recorder = Arc::clone(&fires);
    let timer = ThreadpoolTimer::new(
        move |firing| {
            let seen = recorder.count();
            recorder.record();
            if seen + 1 < 3 {
                // Only the second request should take effect.
                firing.rearm_after(Duration::from_secs(3_600));
                firing.rearm_after(Duration::from_millis(1));
            }
        },
        None,
    )
    .expect("create timer");

    timer.set_after(Duration::from_millis(1));
    fires.wait_for(3);
    timer.disarm();
    timer.wait();
    assert_eq!(fires.count(), 3);
}

#[test]
fn a_callback_that_does_not_rearm_stops() {
    let (timer, fires) = counting_timer();
    timer.set_after(Duration::from_millis(1));
    fires.wait_for(1);
    timer.wait();
    std::thread::sleep(Duration::from_millis(60));
    assert_eq!(fires.count(), 1);
}

#[test]
fn rearm_at_accepts_an_absolute_instant() {
    let fires = Fires::new();
    let recorder = Arc::clone(&fires);
    let rearmed = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&rearmed);

    let timer = ThreadpoolTimer::new(
        move |firing| {
            recorder.record();
            if counter.fetch_add(1, Ordering::SeqCst) == 0 {
                firing.rearm_at(SystemTime::now() + Duration::from_millis(5));
            }
        },
        None,
    )
    .expect("create timer");

    timer.set_after(Duration::from_millis(1));
    fires.wait_for(2);
    timer.disarm();
    timer.wait();
    assert_eq!(fires.count(), 2);
}

// --- absolute due times ---

#[test]
fn set_at_a_past_instant_fires_immediately() {
    let (timer, fires) = counting_timer();
    timer.set_at(SystemTime::UNIX_EPOCH);
    fires.wait_for(1);
    timer.wait();
    assert_eq!(fires.count(), 1);
}

#[test]
fn set_at_a_near_future_instant_fires() {
    let (timer, fires) = counting_timer();
    timer.set_at(SystemTime::now() + Duration::from_millis(50));
    fires.wait_for(1);
    timer.wait();
    assert_eq!(fires.count(), 1);
}

#[test]
fn set_at_a_far_future_instant_does_not_fire() {
    let (timer, fires) = counting_timer();
    timer.set_at(SystemTime::now() + Duration::from_secs(3_600));
    assert!(timer.is_set());
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(fires.count(), 0, "a far-future timer must not fire yet");
    timer.disarm();
}

// --- disarming ---

#[test]
fn disarming_before_firing_prevents_the_callback() {
    let (timer, fires) = counting_timer();
    timer.set_after(Duration::from_secs(3_600));
    assert!(timer.is_set());
    timer.disarm();
    assert!(!timer.is_set(), "disarm must clear the armed state");
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(fires.count(), 0);
}

#[test]
fn disarming_an_idle_timer_is_a_no_op() {
    let (timer, fires) = counting_timer();
    timer.disarm();
    timer.disarm();
    assert!(!timer.is_set());
    assert_eq!(fires.count(), 0);
}

#[test]
fn cancel_pending_after_disarm_drains_cleanly() {
    let (timer, fires) = counting_timer();
    timer.set_after(Duration::from_millis(1));
    fires.wait_for(1);
    timer.disarm();
    timer.cancel_pending();
    let settled = fires.count();
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(fires.count(), settled);
}

// --- quiescing without dropping ---

/// Build a timer whose callback always re-arms, and whose firing is slow enough
/// that a caller can reliably act while one is executing.
fn always_rearming_timer() -> (ThreadpoolTimer, Arc<Fires>) {
    let fires = Fires::new();
    let counter = Arc::clone(&fires);
    let timer = ThreadpoolTimer::new(
        move |firing| {
            counter.record();
            std::thread::sleep(Duration::from_millis(60));
            firing.rearm_after(Duration::from_millis(1));
        },
        None,
    )
    .expect("create timer");
    (timer, fires)
}

/// Wait until a callback has been entered, so the caller acts while one runs.
fn wait_until_firing(fires: &Fires) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while fires.count() == 0 {
        assert!(Instant::now() < deadline, "the timer never fired");
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// `stop_and_drain` must leave a self-re-arming timer idle, which is what
/// distinguishes it from `disarm` plus a drain.
#[test]
fn stop_and_drain_quiesces_a_self_rearming_timer() {
    let (timer, fires) = always_rearming_timer();

    timer.set_after(Duration::ZERO);
    wait_until_firing(&fires);
    timer.stop_and_drain();

    let settled = fires.count();
    assert!(!timer.is_set(), "the timer still has a due time");
    std::thread::sleep(Duration::from_millis(150));
    assert_eq!(
        fires.count(),
        settled,
        "the timer kept firing after stop_and_drain"
    );
}

/// Quiescence alone does not prove *this* type delivered it: dropping queued
/// callbacks happens to leave the timer idle as well, so the observable outcome
/// is the same either way. This asserts the mechanism instead -- the in-flight
/// callback's deferred re-arm must be actively suppressed, so quiescence rests
/// on our own lock rather than on how the pool treats an arming that races a
/// cancel, which no SDK contract promises.
#[test]
fn stop_and_drain_suppresses_the_in_flight_rearm() {
    let outcomes = Arc::new(Mutex::new(Vec::new()));
    let (timer, fires) = always_rearming_timer();
    timer.observe_rearms(&outcomes);

    timer.set_after(Duration::ZERO);
    wait_until_firing(&fires);
    timer.stop_and_drain();

    let applied = outcomes.lock().unwrap().clone();
    assert!(
        !applied.is_empty(),
        "the callback made no re-arm request to suppress"
    );
    assert!(
        applied.iter().all(|&applied| !applied),
        "a re-arm was applied during stop_and_drain: {applied:?}"
    );
}

/// The contrast that motivates the method: `disarm` followed by `wait` cannot
/// quiesce the same timer, because the re-arm is applied after the callback
/// returns -- which is after the disarm.
#[test]
fn disarm_then_wait_does_not_quiesce_a_self_rearming_timer() {
    let (timer, fires) = always_rearming_timer();

    timer.set_after(Duration::ZERO);
    wait_until_firing(&fires);
    timer.disarm();
    timer.wait();

    assert!(
        timer.is_set(),
        "expected the deferred re-arm to have re-armed the timer"
    );
    timer.stop_and_drain();
}

/// The suppression must be lifted on return, or the timer would be permanently
/// dead rather than merely idle.
#[test]
fn a_timer_is_reusable_after_stop_and_drain() {
    let (timer, fires) = always_rearming_timer();

    timer.set_after(Duration::ZERO);
    wait_until_firing(&fires);
    timer.stop_and_drain();
    let settled = fires.count();

    timer.set_after(Duration::ZERO);
    let deadline = Instant::now() + Duration::from_secs(30);
    while fires.count() == settled {
        assert!(
            Instant::now() < deadline,
            "the timer never fired again after stop_and_drain"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    timer.stop_and_drain();
}

/// Concurrent callers must not lift one another's suppression, which is why the
/// suppression is a count rather than a flag.
#[test]
fn concurrent_stop_and_drain_calls_all_quiesce() {
    let (timer, fires) = always_rearming_timer();
    let timer = Arc::new(timer);

    timer.set_after(Duration::ZERO);
    wait_until_firing(&fires);

    let callers: Vec<_> = (0..4)
        .map(|_| {
            let timer = Arc::clone(&timer);
            std::thread::spawn(move || timer.stop_and_drain())
        })
        .collect();
    for caller in callers {
        caller.join().expect("stop_and_drain thread");
    }

    let settled = fires.count();
    assert!(!timer.is_set(), "the timer still has a due time");
    std::thread::sleep(Duration::from_millis(150));
    assert_eq!(
        fires.count(),
        settled,
        "the timer kept firing after concurrent stop_and_drain calls"
    );
}

#[test]
fn stop_and_drain_on_an_idle_timer_is_a_no_op() {
    let (timer, fires) = counting_timer();
    timer.stop_and_drain();
    assert!(!timer.is_set());
    assert_eq!(fires.count(), 0);
    // Still usable afterwards.
    timer.set_after(Duration::ZERO);
    fires.wait_for(1);
    timer.stop_and_drain();
}

// --- destruction ---

/// Outside of teardown, a deferred re-arm is applied. This is the control for
/// [`rearming_during_teardown_is_suppressed`], which asserts the opposite.
#[test]
fn rearming_outside_teardown_is_applied() {
    let outcomes = Arc::new(Mutex::new(Vec::new()));
    let fires = Fires::new();
    let counter = Arc::clone(&fires);

    let timer = ThreadpoolTimer::new(
        move |firing| {
            // Re-arm only on the first firing, so this terminates.
            if counter.count() == 0 {
                firing.rearm_after(Duration::from_millis(1));
            }
            counter.record();
        },
        None,
    )
    .expect("create timer");
    timer.observe_rearms(&outcomes);

    timer.set_after(Duration::from_millis(1));
    fires.wait_for(2);
    timer.disarm();
    timer.cancel_pending();

    assert_eq!(*outcomes.lock().unwrap(), vec![true]);
}

/// A callback that asks to re-arm while `Drop` is tearing down must not leave a
/// due time installed behind it. Deferring the re-arm to after the callback
/// returns -- which is what makes the delay run from the end of the firing --
/// moves it past `Drop`'s disarm, so the drain could otherwise complete with the
/// timer armed and the close would race a freshly queued callback.
#[test]
fn rearming_during_teardown_is_suppressed() {
    let outcomes = Arc::new(Mutex::new(Vec::new()));
    let fires = Fires::new();
    let counter = Arc::clone(&fires);

    let elapsed = Instant::now();
    {
        let timer = ThreadpoolTimer::new(
            move |firing| {
                counter.record();
                firing.rearm_after(Duration::from_millis(1));
                // Give the dropping thread time to flag teardown, so the
                // deferred re-arm lands during it rather than before it.
                std::thread::sleep(Duration::from_millis(200));
            },
            None,
        )
        .expect("create timer");
        timer.observe_rearms(&outcomes);

        timer.set_after(Duration::from_millis(1));
        fires.wait_for(1);
        // Drop here, concurrently with the callback's deferred re-arm.
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

/// Drop must wait for an executing callback before freeing the context, or the
/// captured closure would be freed while still running.
#[test]
fn drop_waits_for_an_executing_callback() {
    let done = Arc::new(AtomicUsize::new(0));
    let flag = Arc::clone(&done);
    let started = Fires::new();
    let entered = Arc::clone(&started);

    {
        let timer = ThreadpoolTimer::new(
            move |_firing| {
                entered.record();
                std::thread::sleep(Duration::from_millis(30));
                flag.fetch_add(1, Ordering::SeqCst);
            },
            None,
        )
        .expect("create timer");
        timer.set_after(Duration::from_millis(1));
        // Only return once the callback is genuinely running, so Drop has
        // something in flight to wait for.
        started.wait_for(1);
        // Drop here.
    }

    assert_eq!(
        done.load(Ordering::SeqCst),
        1,
        "Drop returned while a callback was still executing"
    );
}

/// Dropping a timer whose callback re-arms must terminate: Drop disarms first.
#[test]
fn drop_of_a_self_rearming_timer_terminates() {
    let started = Instant::now();
    {
        let fires = Fires::new();
        let recorder = Arc::clone(&fires);
        let timer = ThreadpoolTimer::new(
            move |firing| {
                recorder.record();
                firing.rearm_after(Duration::from_millis(1));
            },
            None,
        )
        .expect("create timer");
        timer.set_after(Duration::from_millis(1));
        fires.wait_for(3);
        // Drop here, with the callback still re-arming itself.
    }
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "dropping a self-rearming timer appears to have hung"
    );
}

#[test]
fn drop_while_armed_but_not_yet_fired_is_clean() {
    let (timer, fires) = counting_timer();
    timer.set_after(Duration::from_secs(3_600));
    drop(timer);
    assert_eq!(fires.count(), 0);
}

// --- callback state and environment ---

#[test]
fn the_callback_may_own_heap_state() {
    let data = Arc::new(vec![1_u64, 2, 3, 4, 5]);
    let sum = Arc::new(AtomicUsize::new(0));
    let total = Arc::clone(&sum);
    let fires = Fires::new();
    let recorder = Arc::clone(&fires);

    let timer = ThreadpoolTimer::new(
        move |_firing| {
            total.fetch_add(data.iter().sum::<u64>() as usize, Ordering::SeqCst);
            recorder.record();
        },
        None,
    )
    .expect("create timer");

    timer.set_after(Duration::from_millis(1));
    fires.wait_for(1);
    timer.wait();
    assert_eq!(sum.load(Ordering::SeqCst), 15);
}

#[test]
fn a_timer_runs_on_a_private_pool() {
    // Declared before the timer so it outlives it.
    let pool = ThreadpoolPool::new().expect("create pool");
    let mut env = CallbackEnviron::new();
    env.set_pool(&pool);

    let fires = Fires::new();
    let recorder = Arc::clone(&fires);
    let timer = ThreadpoolTimer::new(move |_firing| recorder.record(), Some(&mut env))
        .expect("create timer");

    timer.set_after(Duration::from_millis(1));
    fires.wait_for(1);
    timer.wait();
    assert_eq!(fires.count(), 1);
}

#[test]
fn a_timer_is_send_and_sync() {
    fn assert_send<T: Send>() {}
    fn assert_sync<T: Sync>() {}
    assert_send::<ThreadpoolTimer>();
    assert_sync::<ThreadpoolTimer>();
}

/// Arming from another thread must work, since the type advertises `Sync`.
#[test]
fn a_timer_can_be_armed_from_another_thread() {
    let (timer, fires) = counting_timer();
    std::thread::scope(|scope| {
        let timer = &timer;
        scope.spawn(move || timer.set_after(Duration::from_millis(1)));
    });
    fires.wait_for(1);
    timer.wait();
    assert_eq!(fires.count(), 1);
}

// --- coalescing window ---

#[test]
fn a_coalescing_window_still_fires() {
    let (timer, fires) = counting_timer();
    timer.set_after_with_window(Duration::from_millis(1), Duration::from_millis(20));
    fires.wait_for(1);
    timer.wait();
    assert_eq!(fires.count(), 1);
}
