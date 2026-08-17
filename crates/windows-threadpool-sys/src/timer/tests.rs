// Copyright (c) 2026 Mike Grier
//! Unit tests for thread-pool timers.
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
struct Fires {
    count: Mutex<usize>,
    fired: Condvar,
}

impl Fires {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            count: Mutex::new(0),
            fired: Condvar::new(),
        })
    }

    fn record(&self) {
        let mut count = self.count.lock().expect("record a fire");
        *count += 1;
        self.fired.notify_all();
    }

    fn count(&self) -> usize {
        *self.count.lock().expect("read the count")
    }

    /// Block until at least `target` callbacks have run, failing rather than
    /// hanging if they never do.
    fn wait_for(&self, target: usize) {
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
    let timer = ThreadpoolTimer::new(move || recorder.record(), None).expect("create timer");
    (timer, fires)
}

// --- creation ---

#[test]
fn new_timer_succeeds() {
    assert!(ThreadpoolTimer::new(|| {}, None).is_ok());
}

#[test]
fn new_timer_with_env_succeeds() {
    let mut env = CallbackEnviron::new();
    assert!(ThreadpoolTimer::new(|| {}, Some(&mut env)).is_ok());
}

#[test]
fn new_timer_is_idle() {
    let timer = ThreadpoolTimer::new(|| {}, None).expect("create timer");
    assert!(!timer.is_set(), "a fresh timer must not be armed");
}

#[test]
fn drop_without_arming_is_clean() {
    let _timer = ThreadpoolTimer::new(|| {}, None).expect("create timer");
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
fn a_one_shot_timer_stays_set_after_firing_until_disarmed() {
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

// --- periodic firing ---

#[test]
fn set_periodic_fires_repeatedly() {
    let (timer, fires) = counting_timer();
    timer.set_periodic(Duration::from_millis(1), Duration::from_millis(2));
    fires.wait_for(5);
    timer.disarm();
    timer.wait();
    assert!(
        fires.count() >= 5,
        "a periodic timer must keep firing; saw {}",
        fires.count()
    );
}

#[test]
fn a_periodic_timer_stays_set_between_firings() {
    let (timer, fires) = counting_timer();
    timer.set_periodic(Duration::from_millis(1), Duration::from_millis(50));
    fires.wait_for(1);
    assert!(timer.is_set(), "a periodic timer stays armed after firing");
    timer.disarm();
    timer.wait();
}

#[test]
fn a_zero_period_behaves_as_one_shot() {
    let (timer, fires) = counting_timer();
    timer.set_periodic(Duration::from_millis(1), Duration::ZERO);
    fires.wait_for(1);
    timer.wait();
    // A zero period must not requeue the callback, which is observable in the
    // count rather than in `is_set` -- expiry never clears the due time.
    let settled = fires.count();
    std::thread::sleep(Duration::from_millis(60));
    assert_eq!(fires.count(), settled, "a zero period must not rearm");
    assert_eq!(settled, 1);
}

#[test]
fn disarming_a_periodic_timer_stops_further_firing() {
    let (timer, fires) = counting_timer();
    timer.set_periodic(Duration::from_millis(1), Duration::from_millis(2));
    fires.wait_for(3);
    timer.disarm();
    timer.wait();

    let settled = fires.count();
    std::thread::sleep(Duration::from_millis(60));
    assert_eq!(
        fires.count(),
        settled,
        "no callback may run after disarm and drain"
    );
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
    timer.set_periodic(Duration::from_millis(1), Duration::from_millis(1));
    fires.wait_for(2);
    timer.disarm();
    timer.cancel_pending();
    let settled = fires.count();
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(fires.count(), settled);
}

// --- destruction ---

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
            move || {
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

/// Dropping a periodic timer must terminate: `Drop` disarms before draining, so
/// the timer cannot requeue itself forever.
#[test]
fn drop_of_a_periodic_timer_terminates() {
    let started = Instant::now();
    {
        let (timer, fires) = counting_timer();
        timer.set_periodic(Duration::from_millis(1), Duration::from_millis(1));
        fires.wait_for(3);
        // Drop here, with the timer still armed and firing.
    }
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "dropping a periodic timer appears to have hung"
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
        move || {
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
    let timer =
        ThreadpoolTimer::new(move || recorder.record(), Some(&mut env)).expect("create timer");

    timer.set_after(Duration::from_millis(1));
    fires.wait_for(1);
    timer.wait();
    assert_eq!(fires.count(), 1);
}

/// A panicking callback must be contained at the FFI boundary, and must not stop
/// a periodic timer from continuing.
///
/// The caught panic prints to stderr; that output is expected.
#[test]
fn a_panicking_callback_is_contained() {
    let fires = Fires::new();
    let recorder = Arc::clone(&fires);
    let timer = ThreadpoolTimer::new(
        move || {
            recorder.record();
            panic!("timer callback panics on purpose");
        },
        None,
    )
    .expect("create timer");

    timer.set_periodic(Duration::from_millis(1), Duration::from_millis(2));
    fires.wait_for(3);
    timer.disarm();
    timer.wait();
    assert!(fires.count() >= 3, "panics must not stop the timer");
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
    timer.set_after_with_window(
        Duration::from_millis(1),
        Duration::ZERO,
        Duration::from_millis(20),
    );
    fires.wait_for(1);
    timer.wait();
    assert_eq!(fires.count(), 1);
}
