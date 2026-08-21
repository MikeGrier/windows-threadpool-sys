// Copyright (c) 2026 Mike Grier
//! Unit tests for cleanup groups.
//!
//! The contract under test is ownership: members are released in bulk, their
//! contexts and handles are freed only after that release, and the borrow
//! checker prevents a member outliving it. The compile-time half of that is
//! covered by a `compile_fail` doc test on the module.

use std::os::windows::io::AsRawHandle;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
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

/// The `create_*` methods take `&self`, so a group can gain new members after a
/// release has returned. Those members must be released like any others: a
/// release that latched after its first run would skip them, leaking their
/// contexts and leaving the group to be closed with members still live.
#[test]
fn members_created_after_a_release_are_still_released() {
    let mut group = CleanupGroup::new().expect("create group");
    {
        let work = group.create_work(|| {}, None).expect("create work");
        work.submit();
        work.wait();
    }
    group.close_members(false);
    assert_eq!(
        group.owned_resources(),
        0,
        "the first batch was not released"
    );

    {
        let work = group.create_work(|| {}, None).expect("create second work");
        work.submit();
        work.wait();
    }
    assert_eq!(
        group.owned_resources(),
        1,
        "the group is not tracking the second batch"
    );

    group.close_members(false);
    assert_eq!(
        group.owned_resources(),
        0,
        "the second batch was not released"
    );
}

/// The same reuse across every member kind, and left to `Drop` rather than an
/// explicit close, which is the path that would otherwise close the group with
/// live members.
#[test]
fn a_reused_group_releases_its_second_batch_on_drop() {
    let ran = Ran::new();
    {
        let mut group = CleanupGroup::new().expect("create group");
        {
            let work = group.create_work(|| {}, None).expect("create work");
            work.submit();
            work.wait();
        }
        group.close_members(true);

        {
            let counter = Arc::clone(&ran);
            let timer = group
                .create_timer(move |_| counter.record(), None)
                .expect("create timer member");
            timer.set_after(Duration::from_millis(1));
            let periodic = group
                .create_periodic_timer(Duration::from_millis(1), |_| {}, None)
                .expect("create periodic member");
            periodic.start_after(Duration::from_millis(1));
            let wait = group
                .create_wait(event(), |_| {}, None)
                .expect("create wait member");
            wait.arm(None);
        }
        assert_eq!(
            group.owned_resources(),
            4,
            "the group is not tracking the second batch"
        );
        // Dropped here without another close_members.
    }
    // Reaching here without a hang or a crash is the assertion: Drop released
    // the second batch before closing the group.
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

/// A two-phase gate that parks the first callback inside its body until the
/// test has begun the group's release, so a re-arm requested from the callback
/// happens while `CloseThreadpoolCleanupGroupMembers` is waiting on it.
struct RearmGate {
    entered: (Mutex<bool>, Condvar),
    release: (Mutex<bool>, Condvar),
    parked_once: AtomicBool,
}

impl RearmGate {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            entered: (Mutex::new(false), Condvar::new()),
            release: (Mutex::new(false), Condvar::new()),
            parked_once: AtomicBool::new(false),
        })
    }

    /// First call announces entry and blocks until `let_go`; later calls return
    /// at once, so only the first firing is held mid-flight.
    fn park_first(&self) {
        if self.parked_once.swap(true, Ordering::SeqCst) {
            return;
        }
        let (m, c) = &self.entered;
        *m.lock().expect("entered") = true;
        c.notify_all();
        let (m, c) = &self.release;
        let mut go = m.lock().expect("release");
        while !*go {
            go = c.wait(go).expect("release");
        }
    }

    fn await_entered(&self) {
        let (m, c) = &self.entered;
        let mut seen = m.lock().expect("entered");
        while !*seen {
            seen = c.wait(seen).expect("entered");
        }
    }

    fn let_go(&self) {
        let (m, c) = &self.release;
        *m.lock().expect("release") = true;
        c.notify_all();
    }
}

/// Releasing a group while a self-re-arming timer callback is mid-flight must
/// leave the timer quiescent. `CloseThreadpoolCleanupGroupMembers` waits for the
/// executing callback but does not suppress the deferred re-arm it requests
/// through [`TimerFiring::rearm_after`]; the group must suppress and disarm each
/// member first, as an individual timer's own `Drop` does. Without that the
/// re-arm re-arms an object being torn down and queues a callback against a
/// context the group is about to free. Run under a bounded thread so a
/// regression surfaces as a failure rather than a wedged run.
///
/// [`TimerFiring::rearm_after`]: crate::timer::TimerFiring::rearm_after
#[test]
fn releasing_a_group_while_a_timer_callback_rearms_leaves_it_quiescent() {
    let gate = RearmGate::new();
    let count = Arc::new(AtomicUsize::new(0));

    let mut group = CleanupGroup::new().expect("create group");
    {
        let cb_gate = Arc::clone(&gate);
        let cb_count = Arc::clone(&count);
        let timer = group
            .create_timer(
                move |firing| {
                    cb_count.fetch_add(1, Ordering::SeqCst);
                    cb_gate.park_first();
                    // Ask to keep firing; the teardown must suppress this.
                    firing.rearm_after(Duration::from_millis(1));
                },
                None,
            )
            .expect("create timer");
        timer.set_after(Duration::from_millis(1));
    }

    gate.await_entered();
    let releaser = {
        let gate = Arc::clone(&gate);
        std::thread::spawn(move || {
            // Give close_members time to reach its wait on the parked callback.
            std::thread::sleep(Duration::from_millis(50));
            gate.let_go();
        })
    };

    group.close_members(false);
    releaser.join().expect("releaser");

    // The suppressed re-arm must not have re-armed a torn-down object.
    let after_release = count.load(Ordering::SeqCst);
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(
        count.load(Ordering::SeqCst),
        after_release,
        "the timer re-armed past the group's release",
    );
}

/// The wait counterpart of the timer teardown race: a wait callback that
/// re-arms through [`WaitActivation::rearm`] while the group is releasing its
/// members must be suppressed, not applied to an object being torn down.
///
/// [`WaitActivation::rearm`]: crate::wait::WaitActivation::rearm
#[test]
fn releasing_a_group_while_a_wait_callback_rearms_leaves_it_quiescent() {
    let gate = RearmGate::new();
    let count = Arc::new(AtomicUsize::new(0));
    // A manual-reset event stays signalled, so a re-arm re-fires immediately.
    let handle = event();

    let mut group = CleanupGroup::new().expect("create group");
    {
        let cb_gate = Arc::clone(&gate);
        let cb_count = Arc::clone(&count);
        let wait = group
            .create_wait(
                handle,
                move |activation| {
                    cb_count.fetch_add(1, Ordering::SeqCst);
                    cb_gate.park_first();
                    // Ask to keep watching; the teardown must suppress this.
                    activation.rearm(None);
                },
                None,
            )
            .expect("create wait");
        signal(wait.handle());
        wait.arm(None);
    }

    gate.await_entered();
    let releaser = {
        let gate = Arc::clone(&gate);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            gate.let_go();
        })
    };

    group.close_members(false);
    releaser.join().expect("releaser");

    let after_release = count.load(Ordering::SeqCst);
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(
        count.load(Ordering::SeqCst),
        after_release,
        "the wait re-armed past the group's release",
    );
}

// --- custom-close wait targets in a group (M17) ---

/// Create a real event and hand it over with a caller-supplied close routine.
///
/// Mirrors the helper in the wait unit tests: a fresh event stands in for a
/// handle like `FindFirstChangeNotification`'s, which must be closed with
/// something other than `CloseHandle`.
///
/// SAFETY: the returned handle is a fresh event -- a supported wait target --
/// and ownership transfers into the wrapper, which is what `close` will close.
unsafe fn custom_event(close: crate::wait::WaitCloseFn) -> WaitableHandle {
    // SAFETY: an unnamed, manual-reset, initially-unsignalled event with default
    // security attributes; all pointer arguments are null by design.
    let raw = unsafe {
        windows_sys::Win32::System::Threading::CreateEventW(
            std::ptr::null(),
            1, // manual reset
            0, // initially unsignalled
            std::ptr::null(),
        )
    };
    assert!(!raw.is_null(), "CreateEventW failed");
    // SAFETY: forwarded from this function's own contract.
    unsafe { WaitableHandle::assume_waitable_with(raw, close) }
}

#[test]
fn group_release_runs_a_custom_closer_exactly_once() {
    // Each test owns its statics, so the counts stay correct even under
    // `cargo test`, which runs tests as threads in one process.
    static CLOSES: AtomicUsize = AtomicUsize::new(0);

    unsafe extern "system" fn close(handle: windows_sys::Win32::Foundation::HANDLE) -> i32 {
        CLOSES.fetch_add(1, Ordering::SeqCst);
        // SAFETY: the group owned this event and has released its members, so
        // the pool is no longer watching it.
        unsafe { windows_sys::Win32::Foundation::CloseHandle(handle) }
    }

    let mut group = CleanupGroup::new().expect("create group");

    // SAFETY: a fresh event is a supported wait target, exclusively owned here.
    let handle = unsafe { custom_event(close) };
    let member = group
        .create_wait(handle, |_| {}, None)
        .expect("create wait");
    member.arm(None);
    assert_eq!(
        CLOSES.load(Ordering::SeqCst),
        0,
        "not closed while a member"
    );

    group.close_members(false);
    assert_eq!(
        CLOSES.load(Ordering::SeqCst),
        1,
        "releasing the group must run the custom closer exactly once"
    );

    // Dropping the group must not close it a second time.
    drop(group);
    assert_eq!(CLOSES.load(Ordering::SeqCst), 1, "closed exactly once");
}

#[test]
fn group_drop_runs_a_custom_closer_exactly_once() {
    static CLOSES: AtomicUsize = AtomicUsize::new(0);

    unsafe extern "system" fn close(handle: windows_sys::Win32::Foundation::HANDLE) -> i32 {
        CLOSES.fetch_add(1, Ordering::SeqCst);
        // SAFETY: as above.
        unsafe { windows_sys::Win32::Foundation::CloseHandle(handle) }
    }

    {
        let group = CleanupGroup::new().expect("create group");
        // SAFETY: a fresh event is a supported wait target, exclusively owned here.
        let handle = unsafe { custom_event(close) };
        let member = group
            .create_wait(handle, |_| {}, None)
            .expect("create wait");
        member.arm(None);
        assert_eq!(
            CLOSES.load(Ordering::SeqCst),
            0,
            "not closed while a member"
        );
    }

    assert_eq!(
        CLOSES.load(Ordering::SeqCst),
        1,
        "dropping the group must run the custom closer exactly once"
    );
}

#[test]
fn group_release_runs_a_custom_closer_only_after_draining() {
    static CLOSES: AtomicUsize = AtomicUsize::new(0);

    unsafe extern "system" fn close(handle: windows_sys::Win32::Foundation::HANDLE) -> i32 {
        CLOSES.fetch_add(1, Ordering::SeqCst);
        // SAFETY: as above.
        unsafe { windows_sys::Win32::Foundation::CloseHandle(handle) }
    }

    let mut group = CleanupGroup::new().expect("create group");

    let started = Ran::new();
    let entered = Arc::clone(&started);
    // What the callback saw just before returning. Non-zero would mean the
    // handle was closed while a callback was still executing.
    let seen_at_exit = Arc::new(AtomicUsize::new(usize::MAX));
    let recorder = Arc::clone(&seen_at_exit);

    // SAFETY: a fresh event is a supported wait target, exclusively owned here.
    let handle = unsafe { custom_event(close) };
    let member = group
        .create_wait(
            handle,
            move |_| {
                entered.record();
                // Stay inside the callback so the release below is genuinely
                // draining rather than finding the callback already finished.
                std::thread::sleep(Duration::from_millis(100));
                recorder.store(CLOSES.load(Ordering::SeqCst), Ordering::SeqCst);
            },
            None,
        )
        .expect("create wait");

    member.arm(None);
    // SAFETY: the member owns a live event and is still armed.
    let ok = unsafe { SetEvent(member.handle().as_raw_handle()) };
    assert_ne!(ok, 0, "SetEvent failed");
    // Only return once the callback is actually running.
    started.wait_for(1);

    let entered_release = Instant::now();
    group.close_members(false);
    let blocked_for = entered_release.elapsed();

    // Without this the test could pass vacuously: if the callback had already
    // finished before the release, "closed after the callback" would be true
    // for free. The release is entered microseconds after the callback starts
    // its 100ms dwell, so one that really drains cannot return promptly.
    assert!(
        blocked_for >= Duration::from_millis(50),
        "close_members returned in {blocked_for:?}, so it did not drain a running callback"
    );
    assert_eq!(
        seen_at_exit.load(Ordering::SeqCst),
        0,
        "the handle was closed while a callback was still executing"
    );
    assert_eq!(
        CLOSES.load(Ordering::SeqCst),
        1,
        "the custom closer must run exactly once, after the drain"
    );
}

#[test]
fn cancelling_pending_still_runs_a_custom_closer_exactly_once() {
    static CLOSES: AtomicUsize = AtomicUsize::new(0);

    unsafe extern "system" fn close(handle: windows_sys::Win32::Foundation::HANDLE) -> i32 {
        CLOSES.fetch_add(1, Ordering::SeqCst);
        // SAFETY: as above.
        unsafe { windows_sys::Win32::Foundation::CloseHandle(handle) }
    }

    let mut group = CleanupGroup::new().expect("create group");

    // SAFETY: a fresh event is a supported wait target, exclusively owned here.
    let handle = unsafe { custom_event(close) };
    let member = group
        .create_wait(handle, |_| {}, None)
        .expect("create wait");
    member.arm(None);
    // Signal it, then cancel: whether the callback runs or is dropped, the
    // handle must still be closed once, by the caller's routine.
    // SAFETY: the member owns a live event and is still armed.
    let ok = unsafe { SetEvent(member.handle().as_raw_handle()) };
    assert_ne!(ok, 0, "SetEvent failed");

    group.close_members(true);
    assert_eq!(
        CLOSES.load(Ordering::SeqCst),
        1,
        "cancelling pending callbacks must still close the handle exactly once"
    );
}

#[test]
fn a_group_releases_default_and_custom_close_members_together() {
    // The custom-close seam must not disturb the default path when both kinds
    // of member are released by the same call.
    static CLOSES: AtomicUsize = AtomicUsize::new(0);

    unsafe extern "system" fn close(handle: windows_sys::Win32::Foundation::HANDLE) -> i32 {
        CLOSES.fetch_add(1, Ordering::SeqCst);
        // SAFETY: as above.
        unsafe { windows_sys::Win32::Foundation::CloseHandle(handle) }
    }

    let mut group = CleanupGroup::new().expect("create group");

    let ran = Ran::new();
    let recorder = Arc::clone(&ran);

    // SAFETY: a fresh event is a supported wait target, exclusively owned here.
    let custom = unsafe { custom_event(close) };
    let custom_member = group
        .create_wait(custom, |_| {}, None)
        .expect("custom wait");
    custom_member.arm(None);

    let owned = WaitableHandle::event(true, false).expect("create an event");
    let owned_member = group
        .create_wait(owned, move |_| recorder.record(), None)
        .expect("owned wait");
    owned_member.arm(None);
    // SAFETY: the member owns a live event and is still armed.
    let ok = unsafe { SetEvent(owned_member.handle().as_raw_handle()) };
    assert_ne!(ok, 0, "SetEvent failed");
    ran.wait_for(1);

    group.close_members(false);

    assert_eq!(
        CLOSES.load(Ordering::SeqCst),
        1,
        "the custom-close member is closed exactly once"
    );
    assert_eq!(
        group.owned_resources(),
        0,
        "both members' resources are released"
    );
}
