// Copyright (c) 2026 Mike Grier
//! Unit tests for cleanup groups.
//!
//! The contract under test is ownership: members are released in bulk, their
//! contexts and handles are freed only after that release, and the borrow
//! checker prevents a member outliving it. The compile-time half of that is
//! covered by a `compile_fail` doc test on the module.

use std::os::windows::io::AsRawHandle;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use windows_sys::Win32::System::Threading::SetEvent;

use crate::callback_env::CallbackEnviron;
use crate::cleanup_group::CleanupGroup;
use crate::pool::ThreadpoolPool;
use crate::wait::WaitableHandle;

/// Upper bound for waiting on a callback a member really should deliver.
const CALLBACK_TIMEOUT: Duration = Duration::from_secs(30);

/// Counts callbacks and lets a test block until a target count is reached.
struct Ran {
    count: Mutex<usize>,
    fired: Condvar,
}

impl Ran {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            count: Mutex::new(0),
            fired: Condvar::new(),
        })
    }

    fn record(&self) {
        let mut count = self.count.lock().expect("record");
        *count += 1;
        self.fired.notify_all();
    }

    fn count(&self) -> usize {
        *self.count.lock().expect("read")
    }

    fn wait_for(&self, target: usize) {
        let count = self.count.lock().expect("await");
        let (count, timeout) = self
            .fired
            .wait_timeout_while(count, CALLBACK_TIMEOUT, |count| *count < target)
            .expect("await");
        assert!(
            !timeout.timed_out(),
            "timed out waiting for {target} callback(s); saw {count}"
        );
    }
}

/// Create a manual-reset event, initially unsignalled.
fn event() -> WaitableHandle {
    WaitableHandle::event(true, false).expect("create an event")
}

fn signal(handle: std::os::windows::io::BorrowedHandle<'_>) {
    // SAFETY: the handle is a live event owned by the group.
    let ok = unsafe { SetEvent(handle.as_raw_handle()) };
    assert_ne!(ok, 0, "SetEvent failed");
}

// --- creation ---

#[test]
fn new_group_succeeds() {
    assert!(CleanupGroup::new().is_ok());
}

#[test]
fn a_new_group_owns_nothing() {
    let group = CleanupGroup::new().expect("create group");
    assert_eq!(group.owned_resources(), 0);
}

#[test]
fn drop_without_members_is_clean() {
    let _group = CleanupGroup::new().expect("create group");
}

#[test]
fn a_group_is_send_and_sync() {
    fn assert_send<T: Send>() {}
    fn assert_sync<T: Sync>() {}
    assert_send::<CleanupGroup>();
    assert_sync::<CleanupGroup>();
}

// --- members run normally ---

#[test]
fn a_work_member_runs() {
    let ran = Ran::new();
    let recorder = Arc::clone(&ran);
    let mut group = CleanupGroup::new().expect("create group");
    {
        let work = group
            .create_work(move || recorder.record(), None)
            .expect("create work");
        work.submit();
        work.wait();
    }
    group.close_members(false);
    assert_eq!(ran.count(), 1);
}

#[test]
fn a_timer_member_fires() {
    let ran = Ran::new();
    let recorder = Arc::clone(&ran);
    let mut group = CleanupGroup::new().expect("create group");
    {
        let timer = group
            .create_timer(move |_firing| recorder.record(), None)
            .expect("create timer");
        timer.set_after(Duration::from_millis(1));
        ran.wait_for(1);
        timer.wait();
        assert!(timer.is_set(), "expiry does not clear the due time");
    }
    group.close_members(false);
    assert_eq!(ran.count(), 1);
}

#[test]
fn a_timer_member_can_rearm_itself() {
    let ran = Ran::new();
    let recorder = Arc::clone(&ran);
    let mut group = CleanupGroup::new().expect("create group");
    {
        let timer = group
            .create_timer(
                move |firing| {
                    let seen = recorder.count();
                    recorder.record();
                    if seen + 1 < 3 {
                        firing.rearm_after(Duration::from_millis(1));
                    }
                },
                None,
            )
            .expect("create timer");
        timer.set_after(Duration::from_millis(1));
        ran.wait_for(3);
        timer.disarm();
        timer.wait();
    }
    group.close_members(false);
    assert_eq!(ran.count(), 3);
}

#[test]
fn a_periodic_timer_member_ticks() {
    let ran = Ran::new();
    let recorder = Arc::clone(&ran);
    let mut group = CleanupGroup::new().expect("create group");
    {
        let timer = group
            .create_periodic_timer(
                Duration::from_millis(2),
                move |_tick| recorder.record(),
                None,
            )
            .expect("create periodic timer");
        assert_eq!(timer.period(), Duration::from_millis(2));
        timer.start_after(Duration::from_millis(1));
        ran.wait_for(3);
        assert!(timer.is_running());
        timer.stop_and_drain();
    }
    group.close_members(false);
    assert!(ran.count() >= 3);
}

/// The group path must reject the same periods the standalone constructor does,
/// including sub-millisecond ones that would round down to a non-repeating zero.
#[test]
fn a_periodic_timer_member_rejects_a_period_below_the_minimum() {
    let group = CleanupGroup::new().expect("create group");
    for period in [
        Duration::ZERO,
        Duration::from_micros(1),
        Duration::from_micros(999),
    ] {
        let error = group
            .create_periodic_timer(period, |_| {}, None)
            .expect_err("a period below the minimum must be rejected");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput, "{period:?}");
        assert_eq!(group.owned_resources(), 0, "a failed creation owns nothing");
    }
}

#[test]
fn a_wait_member_activates() {
    let ran = Ran::new();
    let recorder = Arc::clone(&ran);
    let mut group = CleanupGroup::new().expect("create group");
    {
        let wait = group
            .create_wait(event(), move |_activation| recorder.record(), None)
            .expect("create wait");
        wait.arm(None);
        signal(wait.handle());
        ran.wait_for(1);
        wait.wait();
    }
    group.close_members(false);
    assert_eq!(ran.count(), 1);
}

// --- bulk release ---

#[test]
fn close_members_releases_every_kind_at_once() {
    let ran = Ran::new();
    let work_ran = Arc::clone(&ran);
    let timer_ran = Arc::clone(&ran);
    let periodic_ran = Arc::clone(&ran);
    let wait_ran = Arc::clone(&ran);

    let mut group = CleanupGroup::new().expect("create group");
    {
        let work = group
            .create_work(move || work_ran.record(), None)
            .expect("create work");
        let timer = group
            .create_timer(move |_| timer_ran.record(), None)
            .expect("create timer");
        let periodic = group
            .create_periodic_timer(
                Duration::from_millis(2),
                move |_| periodic_ran.record(),
                None,
            )
            .expect("create periodic timer");
        let wait = group
            .create_wait(event(), move |_| wait_ran.record(), None)
            .expect("create wait");

        // Five resources: one context per member, plus the wait's handle.
        assert_eq!(group.owned_resources(), 5);

        work.submit();
        work.wait();
        timer.set_after(Duration::from_millis(1));
        periodic.start_after(Duration::from_millis(1));
        wait.arm(None);
        signal(wait.handle());
        ran.wait_for(4);
    }

    group.close_members(true);
    assert_eq!(
        group.owned_resources(),
        0,
        "releasing members must free their contexts and handles"
    );
}

#[test]
fn close_members_is_idempotent() {
    let mut group = CleanupGroup::new().expect("create group");
    {
        let work = group.create_work(|| {}, None).expect("create work");
        work.submit();
        work.wait();
    }
    group.close_members(false);
    assert_eq!(group.owned_resources(), 0);
    group.close_members(false);
    group.close_members(true);
    assert_eq!(group.owned_resources(), 0);
}

#[test]
fn close_members_waits_for_an_executing_callback() {
    let done = Arc::new(AtomicUsize::new(0));
    let flag = Arc::clone(&done);
    let started = Ran::new();
    let entered = Arc::clone(&started);

    let mut group = CleanupGroup::new().expect("create group");
    {
        let work = group
            .create_work(
                move || {
                    entered.record();
                    std::thread::sleep(Duration::from_millis(30));
                    flag.fetch_add(1, Ordering::SeqCst);
                },
                None,
            )
            .expect("create work");
        work.submit();
        // Only return once the callback is genuinely running.
        started.wait_for(1);
    }

    group.close_members(true);
    assert_eq!(
        done.load(Ordering::SeqCst),
        1,
        "close_members returned while a callback was still executing"
    );
}

/// Cancelling on release drops callbacks that have not started; running them
/// instead is the other documented behaviour.
#[test]
fn close_members_can_run_or_cancel_queued_callbacks() {
    for cancel in [false, true] {
        let ran = Ran::new();
        let recorder = Arc::clone(&ran);
        let mut group = CleanupGroup::new().expect("create group");
        {
            let work = group
                .create_work(move || recorder.record(), None)
                .expect("create work");
            for _ in 0..8 {
                work.submit();
            }
        }
        group.close_members(cancel);
        // Whichever mode, the count settles and nothing runs afterwards.
        let settled = ran.count();
        std::thread::sleep(Duration::from_millis(40));
        assert_eq!(ran.count(), settled);
        assert!(settled <= 8);
        if !cancel {
            assert_eq!(settled, 8, "without cancelling, every submission must run");
        }
    }
}

// --- teardown ---

/// Dropping a group without releasing its members must still release them.
#[test]
fn drop_releases_members() {
    let ran = Ran::new();
    let recorder = Arc::clone(&ran);
    {
        let group = CleanupGroup::new().expect("create group");
        let work = group
            .create_work(move || recorder.record(), None)
            .expect("create work");
        work.submit();
        work.wait();
        // Drop the group without calling close_members.
    }
    assert_eq!(ran.count(), 1);
}

/// Dropping a group whose periodic member is still ticking must terminate.
#[test]
fn drop_of_a_group_with_a_running_periodic_member_terminates() {
    let started = Instant::now();
    {
        let ran = Ran::new();
        let recorder = Arc::clone(&ran);
        let group = CleanupGroup::new().expect("create group");
        let timer = group
            .create_periodic_timer(Duration::from_millis(1), move |_| recorder.record(), None)
            .expect("create periodic timer");
        timer.start_after(Duration::from_millis(1));
        ran.wait_for(3);
        // Drop the group with the timer still running.
    }
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "dropping a group with a running periodic member appears to have hung"
    );
}

/// Dropping a group whose wait member is armed must terminate and close the
/// handle the group owns.
#[test]
fn drop_of_a_group_with_an_armed_wait_terminates() {
    let started = Instant::now();
    {
        let group = CleanupGroup::new().expect("create group");
        let wait = group
            .create_wait(event(), |_| {}, None)
            .expect("create wait");
        wait.arm(None);
        // Drop the group with the wait still armed and never signalled.
    }
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "dropping a group with an armed wait appears to have hung"
    );
}

// --- environment handling ---

/// Members run on the pool the caller's environment selected, and the caller's
/// environment is left untouched.
#[test]
fn a_member_uses_the_callers_pool_without_mutating_the_environment() {
    // Declared before the group so it outlives the members.
    let pool = ThreadpoolPool::new().expect("create pool");
    let mut env = CallbackEnviron::new();
    env.set_pool(&pool);
    let pool_value = env.as_inner().Pool;

    let ran = Ran::new();
    let recorder = Arc::clone(&ran);
    let mut group = CleanupGroup::new().expect("create group");
    {
        let work = group
            .create_work(move || recorder.record(), Some(&env))
            .expect("create work");
        work.submit();
        work.wait();
    }
    group.close_members(false);

    assert_eq!(ran.count(), 1);
    assert_eq!(
        env.as_inner().Pool,
        pool_value,
        "the caller's pool selection must survive"
    );
    assert_eq!(
        env.as_inner().CleanupGroup,
        0,
        "the group must not be written into the caller's environment"
    );
}

/// One environment can create members of two different groups, which only works
/// because the environment is copied rather than mutated.
#[test]
fn one_environment_serves_several_groups() {
    let env = CallbackEnviron::new();
    let ran = Ran::new();
    let first_ran = Arc::clone(&ran);
    let second_ran = Arc::clone(&ran);

    let mut first = CleanupGroup::new().expect("create first group");
    let mut second = CleanupGroup::new().expect("create second group");
    {
        let a = first
            .create_work(move || first_ran.record(), Some(&env))
            .expect("create work in first");
        let b = second
            .create_work(move || second_ran.record(), Some(&env))
            .expect("create work in second");
        a.submit();
        b.submit();
        a.wait();
        b.wait();
    }
    first.close_members(false);
    second.close_members(false);
    assert_eq!(ran.count(), 2);
    assert_eq!(env.as_inner().CleanupGroup, 0);
}

// --- resource accounting ---

#[test]
fn each_member_contributes_its_own_resources() {
    let group = CleanupGroup::new().expect("create group");
    assert_eq!(group.owned_resources(), 0);

    let _work = group.create_work(|| {}, None).expect("create work");
    assert_eq!(group.owned_resources(), 1);

    let _timer = group.create_timer(|_| {}, None).expect("create timer");
    assert_eq!(group.owned_resources(), 2);

    let _periodic = group
        .create_periodic_timer(Duration::from_millis(5), |_| {}, None)
        .expect("create periodic timer");
    assert_eq!(group.owned_resources(), 3);

    // A wait contributes two: its context and the handle the group now owns.
    let _wait = group
        .create_wait(event(), |_| {}, None)
        .expect("create wait");
    assert_eq!(group.owned_resources(), 5);
}

#[test]
fn many_members_are_all_released() {
    const MEMBERS: usize = 50;

    let ran = Ran::new();
    let mut group = CleanupGroup::new().expect("create group");
    {
        let mut works = Vec::new();
        for _ in 0..MEMBERS {
            let recorder = Arc::clone(&ran);
            works.push(
                group
                    .create_work(move || recorder.record(), None)
                    .expect("create work"),
            );
        }
        assert_eq!(group.owned_resources(), MEMBERS);
        for work in &works {
            work.submit();
        }
        for work in &works {
            work.wait();
        }
    }
    group.close_members(false);
    assert_eq!(group.owned_resources(), 0);
    assert_eq!(ran.count(), MEMBERS);
}
