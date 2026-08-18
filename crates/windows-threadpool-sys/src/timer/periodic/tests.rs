// Copyright (c) 2026 Mike Grier
//! Unit tests for periodic thread-pool timers.
//!
//! The important assertions here are about repetition and teardown ordering.
//! Cadence itself is only loosely checked, because the pool coalesces timers and
//! a loaded machine can delay any tick.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant, SystemTime};

use crate::callback_env::CallbackEnviron;
use crate::pool::ThreadpoolPool;
use crate::timer::ThreadpoolPeriodicTimer;
use crate::timer::tests::Fires;

fn counting_timer(period: Duration) -> (ThreadpoolPeriodicTimer, Arc<Fires>) {
    let fires = Fires::new();
    let recorder = Arc::clone(&fires);
    let timer = ThreadpoolPeriodicTimer::new(period, move |_tick| recorder.record(), None)
        .expect("create timer");
    (timer, fires)
}

// --- creation ---

#[test]
fn new_periodic_timer_succeeds() {
    assert!(ThreadpoolPeriodicTimer::new(Duration::from_millis(1), |_| {}, None).is_ok());
}

#[test]
fn new_with_env_succeeds() {
    let mut env = CallbackEnviron::new();
    assert!(ThreadpoolPeriodicTimer::new(Duration::from_millis(1), |_| {}, Some(&mut env)).is_ok());
}

/// A zero period would describe a timer that never repeats, which is what
/// `ThreadpoolTimer` is for. Accepting it would recreate exactly the ambiguity that
/// splitting the types removed.
#[test]
fn a_zero_period_is_rejected() {
    let error = ThreadpoolPeriodicTimer::new(Duration::ZERO, |_| {}, None)
        .expect_err("a zero period must be rejected");
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
}

/// The boundary is at one millisecond, because the pool takes the period as
/// whole milliseconds and anything shorter rounds down to zero -- which the pool
/// reads as "do not repeat".
#[test]
fn a_sub_millisecond_period_is_rejected() {
    for micros in [1u64, 100, 500, 999] {
        let error = ThreadpoolPeriodicTimer::new(Duration::from_micros(micros), |_| {}, None)
            .expect_err("a sub-millisecond period must be rejected");
        assert_eq!(
            error.kind(),
            std::io::ErrorKind::InvalidInput,
            "period of {micros}us"
        );
    }
}

#[test]
fn the_shortest_accepted_period_is_one_millisecond() {
    assert_eq!(
        ThreadpoolPeriodicTimer::MIN_PERIOD,
        Duration::from_millis(1)
    );
    assert!(
        ThreadpoolPeriodicTimer::new(ThreadpoolPeriodicTimer::MIN_PERIOD, |_| {}, None).is_ok(),
        "the advertised minimum period must be accepted"
    );
}

/// The guard exists to keep a periodic timer from silently being a one-shot, so
/// this checks the behaviour rather than the guard: the shortest accepted period
/// must actually repeat.
#[test]
fn the_shortest_accepted_period_actually_repeats() {
    let fires = Fires::new();
    let counter = Arc::clone(&fires);
    let timer = ThreadpoolPeriodicTimer::new(
        ThreadpoolPeriodicTimer::MIN_PERIOD,
        move |_| {
            counter.record();
        },
        None,
    )
    .expect("create timer");

    timer.start_after(Duration::ZERO);
    // Fails rather than hangs if the timer only ever fires once, which is what
    // a period that rounded down to zero would do.
    fires.wait_for(3);
    timer.stop_and_drain();

    assert!(
        fires.count() >= 3,
        "a timer at the minimum period is not repeating"
    );
}

#[test]
fn new_timer_is_stopped() {
    let timer =
        ThreadpoolPeriodicTimer::new(Duration::from_millis(1), |_| {}, None).expect("create timer");
    assert!(!timer.is_running(), "a fresh timer must not be running");
}

#[test]
fn the_period_is_reported() {
    let timer = ThreadpoolPeriodicTimer::new(Duration::from_millis(25), |_| {}, None)
        .expect("create timer");
    assert_eq!(timer.period(), Duration::from_millis(25));
}

#[test]
fn drop_without_starting_is_clean() {
    let _timer =
        ThreadpoolPeriodicTimer::new(Duration::from_millis(1), |_| {}, None).expect("create timer");
}

// --- repetition ---

#[test]
fn start_ticks_repeatedly() {
    let (timer, fires) = counting_timer(Duration::from_millis(2));
    timer.start();
    fires.wait_for(5);
    timer.stop_and_drain();
    assert!(
        fires.count() >= 5,
        "a periodic timer must keep ticking; saw {}",
        fires.count()
    );
}

#[test]
fn start_after_controls_only_the_first_tick() {
    let (timer, fires) = counting_timer(Duration::from_millis(2));
    let started = Instant::now();
    timer.start_after(Duration::from_millis(80));
    fires.wait_for(1);
    let first = started.elapsed();
    // The first tick honours the explicit delay, not the period.
    assert!(
        first >= Duration::from_millis(60),
        "the first tick came after only {first:?}, well before the 80ms delay"
    );
    fires.wait_for(3);
    timer.stop_and_drain();
}

#[test]
fn a_running_timer_reports_running() {
    let (timer, fires) = counting_timer(Duration::from_millis(50));
    timer.start_after(Duration::from_millis(1));
    fires.wait_for(1);
    assert!(timer.is_running(), "ticking must not clear the schedule");
    timer.stop_and_drain();
    assert!(!timer.is_running());
}

#[test]
fn start_at_accepts_an_absolute_first_tick() {
    let (timer, fires) = counting_timer(Duration::from_millis(2));
    timer.start_at(SystemTime::now() + Duration::from_millis(10));
    fires.wait_for(3);
    timer.stop_and_drain();
}

#[test]
fn start_with_window_still_ticks() {
    let (timer, fires) = counting_timer(Duration::from_millis(2));
    timer.start_with_window(Duration::from_millis(1), Duration::from_millis(10));
    fires.wait_for(3);
    timer.stop_and_drain();
}

#[test]
fn restarting_replaces_the_previous_schedule() {
    let (timer, fires) = counting_timer(Duration::from_millis(2));
    timer.start_after(Duration::from_secs(3_600));
    timer.start_after(Duration::from_millis(1));
    fires.wait_for(3);
    timer.stop_and_drain();
}

// --- the overlap contract ---

/// The property that distinguishes this type: the pool queues ticks on schedule
/// regardless of whether the previous tick has finished, so a slow callback runs
/// concurrently with itself.
///
/// This is asserted as a capability rather than a guarantee -- the pool is free
/// to schedule as it sees fit -- so the test tolerates not observing overlap
/// while still failing if the timer stops ticking altogether.
#[test]
fn a_slow_callback_may_overlap_itself() {
    let concurrent = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let fires = Fires::new();

    let in_flight = Arc::clone(&concurrent);
    let high_water = Arc::clone(&peak);
    let recorder = Arc::clone(&fires);

    // Ticks are scheduled far faster than the callback can complete.
    let timer = ThreadpoolPeriodicTimer::new(
        Duration::from_millis(2),
        move |_tick| {
            let now = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
            high_water.fetch_max(now, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(25));
            in_flight.fetch_sub(1, Ordering::SeqCst);
            recorder.record();
        },
        None,
    )
    .expect("create timer");

    timer.start_after(Duration::from_millis(1));
    fires.wait_for(4);
    timer.stop_and_drain();

    let observed = peak.load(Ordering::SeqCst);
    assert!(observed >= 1, "the timer must have ticked at all");
    // Document what actually happened; overlap is expected but not mandated.
    if observed == 1 {
        eprintln!(
            "note: the pool did not overlap ticks in this run, though the callback \
             outlasts the period and may do so"
        );
    }
}

// --- stopping ---

#[test]
fn stop_prevents_further_ticks() {
    let (timer, fires) = counting_timer(Duration::from_millis(2));
    timer.start_after(Duration::from_millis(1));
    fires.wait_for(3);
    timer.stop_and_drain();

    let settled = fires.count();
    std::thread::sleep(Duration::from_millis(60));
    assert_eq!(
        fires.count(),
        settled,
        "no tick may run after stop and drain"
    );
}

#[test]
fn stopping_a_stopped_timer_is_a_no_op() {
    let (timer, fires) = counting_timer(Duration::from_millis(2));
    timer.stop();
    timer.stop();
    assert!(!timer.is_running());
    assert_eq!(fires.count(), 0);
}

#[test]
fn a_timer_can_be_restarted_after_stopping() {
    let (timer, fires) = counting_timer(Duration::from_millis(2));
    timer.start_after(Duration::from_millis(1));
    fires.wait_for(2);
    timer.stop_and_drain();
    let after_stop = fires.count();

    timer.start_after(Duration::from_millis(1));
    fires.wait_for(after_stop + 2);
    timer.stop_and_drain();
    assert!(fires.count() > after_stop);
}

/// A tick can stop its own timer, which is how "tick until done" is written.
#[test]
fn a_tick_can_stop_the_timer() {
    let ticks = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&ticks);
    let fires = Fires::new();
    let recorder = Arc::clone(&fires);

    let timer = ThreadpoolPeriodicTimer::new(
        Duration::from_millis(2),
        move |tick| {
            recorder.record();
            if counter.fetch_add(1, Ordering::SeqCst) >= 2 {
                tick.stop();
            }
        },
        None,
    )
    .expect("create timer");

    timer.start_after(Duration::from_millis(1));
    // The timer stops itself; wait for that rather than for a tick count.
    let deadline = Instant::now() + Duration::from_secs(30);
    while timer.is_running() && Instant::now() < deadline {
        std::thread::yield_now();
    }
    assert!(
        !timer.is_running(),
        "the tick should have stopped the timer"
    );

    timer.stop_and_drain();
    let settled = fires.count();
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(fires.count(), settled);
    assert!(settled >= 3);
}

// --- destruction ---

/// Dropping a running timer must terminate: `Drop` stops before draining, so the
/// timer cannot requeue itself forever.
#[test]
fn drop_of_a_running_timer_terminates() {
    let started = Instant::now();
    {
        let (timer, fires) = counting_timer(Duration::from_millis(1));
        timer.start_after(Duration::from_millis(1));
        fires.wait_for(3);
        // Drop here, with the timer still running.
    }
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "dropping a running periodic timer appears to have hung"
    );
}

/// Drop must wait for an executing tick before freeing the context.
#[test]
fn drop_waits_for_an_executing_tick() {
    let done = Arc::new(AtomicUsize::new(0));
    let flag = Arc::clone(&done);
    let started = Fires::new();
    let entered = Arc::clone(&started);

    {
        let timer = ThreadpoolPeriodicTimer::new(
            Duration::from_secs(3_600),
            move |_tick| {
                entered.record();
                std::thread::sleep(Duration::from_millis(30));
                flag.fetch_add(1, Ordering::SeqCst);
            },
            None,
        )
        .expect("create timer");
        // First tick almost immediately, then not again for an hour, so exactly
        // one tick is in flight when Drop runs.
        timer.start_after(Duration::from_millis(1));
        started.wait_for(1);
        // Drop here.
    }

    assert_eq!(
        done.load(Ordering::SeqCst),
        1,
        "Drop returned while a tick was still executing"
    );
}

#[test]
fn drop_while_started_but_not_yet_ticked_is_clean() {
    let (timer, fires) = counting_timer(Duration::from_secs(3_600));
    timer.start();
    drop(timer);
    assert_eq!(fires.count(), 0);
}

// --- callback state, environment, and thread-safety ---

#[test]
fn the_callback_may_own_heap_state() {
    let data = Arc::new(vec![2_u64, 4, 6]);
    let sum = Arc::new(AtomicUsize::new(0));
    let total = Arc::clone(&sum);
    let fires = Fires::new();
    let recorder = Arc::clone(&fires);

    let timer = ThreadpoolPeriodicTimer::new(
        Duration::from_millis(2),
        move |tick| {
            total.fetch_add(data.iter().sum::<u64>() as usize, Ordering::SeqCst);
            recorder.record();
            tick.stop();
        },
        None,
    )
    .expect("create timer");

    timer.start_after(Duration::from_millis(1));
    fires.wait_for(1);
    timer.stop_and_drain();
    assert!(sum.load(Ordering::SeqCst) >= 12);
}

#[test]
fn a_timer_runs_on_a_private_pool() {
    // Declared before the timer so it outlives it.
    let pool = ThreadpoolPool::new().expect("create pool");
    let mut env = CallbackEnviron::new();
    env.set_pool(&pool);

    let fires = Fires::new();
    let recorder = Arc::clone(&fires);
    let timer = ThreadpoolPeriodicTimer::new(
        Duration::from_millis(2),
        move |_tick| recorder.record(),
        Some(&mut env),
    )
    .expect("create timer");

    timer.start_after(Duration::from_millis(1));
    fires.wait_for(3);
    timer.stop_and_drain();
}

/// A panicking tick must be contained at the FFI boundary, and must not stop the
/// timer.
///
/// The caught panic prints to stderr; that output is expected.
#[test]
fn a_panicking_tick_is_contained_and_the_timer_continues() {
    let fires = Fires::new();
    let recorder = Arc::clone(&fires);
    let timer = ThreadpoolPeriodicTimer::new(
        Duration::from_millis(2),
        move |_tick| {
            recorder.record();
            panic!("periodic tick panics on purpose");
        },
        None,
    )
    .expect("create timer");

    timer.start_after(Duration::from_millis(1));
    fires.wait_for(3);
    timer.stop_and_drain();
    assert!(fires.count() >= 3, "panics must not stop the timer");
}

#[test]
fn a_periodic_timer_is_send_and_sync() {
    fn assert_send<T: Send>() {}
    fn assert_sync<T: Sync>() {}
    assert_send::<ThreadpoolPeriodicTimer>();
    assert_sync::<ThreadpoolPeriodicTimer>();
}

#[test]
fn a_timer_can_be_started_from_another_thread() {
    let (timer, fires) = counting_timer(Duration::from_millis(2));
    std::thread::scope(|scope| {
        let timer = &timer;
        scope.spawn(move || timer.start_after(Duration::from_millis(1)));
    });
    fires.wait_for(3);
    timer.stop_and_drain();
}
