// Copyright (c) Mike Grier
//! Unit tests for the self-heal registry.
//!
//! The registry is process-wide and these tests run as threads in one process,
//! so none of them asserts an absolute entry count or that a given pool is
//! absent -- a sibling test creating a pool object would make either flaky.
//! Each asserts a property of the entry it owns.

use crate::callback_env::CallbackEnviron;
use crate::pool::ThreadpoolPool;
/// Only the `self-heal` tests below create objects; the key tests need no pool
/// member, so without the feature this import would be unused.
#[cfg(feature = "self-heal")]
use crate::work::ThreadpoolWork;

#[test]
fn no_environment_means_the_default_pool() {
    assert_eq!(super::key_of(None), 0);
}

#[test]
fn an_environment_with_no_pool_means_the_default_pool() {
    let env = CallbackEnviron::new();
    assert_eq!(super::key_of(Some(&env)), 0);
}

#[test]
fn an_environment_naming_a_pool_yields_that_pool() {
    let pool = ThreadpoolPool::new().expect("create pool");
    let mut env = CallbackEnviron::new();
    env.set_pool(&pool);
    let key = super::key_of(Some(&env));
    assert_ne!(key, 0, "a named pool must not look like the default pool");
    assert_eq!(key, pool.as_raw() as usize);
}

#[cfg(feature = "self-heal")]
mod on {
    use super::*;
    use crate::heal::PoolEntry;
    use std::sync::Arc;

    /// Hold off every tick for the duration of a test's critical section.
    ///
    /// Making a private pool does **not** isolate these tests. `tick` walks the
    /// whole registry, so another test's tick -- or the background healer's,
    /// which starts at the first cancellation anywhere in the process and runs
    /// until it exits -- can clear this test's mark between the cancellation
    /// that set it and the assertion about it. Measured rather than supposed, though
    /// not by the loop that first found it: a review saw
    /// `a_cancelling_group_release_marks_a_wait_members_pool` fail on run 19 of
    /// this module at 32 test threads, and 60 further runs here -- with this
    /// gate disconnected, and again with a thread calling `tick` in a loop --
    /// did not reproduce it. The window is a few microseconds wide, so failing
    /// to land in it settles nothing either way. Placing a tick in the window
    /// by hand does settle it: inserting `tick_inner()` between that test's
    /// `close_members(true)` and its assertion fails it every time, on the
    /// assertion the review named.
    ///
    /// A test that wants to tick calls `crate::heal::tick_inner` while holding
    /// this; `crate::heal::tick` would deadlock on the gate it already has.
    fn gate() -> std::sync::MutexGuard<'static, ()> {
        crate::heal::TICK_GATE
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    /// The entry for a freshly created private pool, with one object on it.
    fn entry_for(pool: &ThreadpoolPool) -> (ThreadpoolWork, Arc<PoolEntry>) {
        let mut env = CallbackEnviron::new();
        env.set_pool(pool);
        let work = ThreadpoolWork::new(|| {}, Some(&mut env)).expect("create work");
        let entry = Arc::clone(
            crate::heal::entries()
                .iter()
                .find(|e| e.key() == pool.as_raw() as usize)
                .expect("the pool was registered when the object was created"),
        );
        (work, entry)
    }

    #[test]
    fn creating_an_object_registers_its_pool() {
        let pool = ThreadpoolPool::new().expect("create pool");
        let (_work, entry) = entry_for(&pool);
        assert_eq!(entry.key(), pool.as_raw() as usize);
    }

    #[test]
    fn the_repair_object_exists_before_it_is_needed() {
        // The load-bearing property: creating a work object was measured not to
        // release a stall, only submitting one is, so the object cannot be made
        // on the healing path.
        let pool = ThreadpoolPool::new().expect("create pool");
        let (_work, entry) = entry_for(&pool);
        assert_ne!(
            entry.repair_work(),
            0,
            "the entry must carry a usable repair object from registration"
        );
    }

    #[test]
    fn a_fresh_entry_owes_no_repair() {
        let pool = ThreadpoolPool::new().expect("create pool");
        let (_work, entry) = entry_for(&pool);
        assert!(!entry.unhealed(), "nothing has cancelled on this pool");
    }

    #[test]
    fn dropping_the_last_object_retires_the_entry() {
        let pool = ThreadpoolPool::new().expect("create pool");
        let key = pool.as_raw() as usize;
        let (work, _entry) = entry_for(&pool);
        drop(work);
        assert!(
            !crate::heal::entries().iter().any(|e| e.key() == key),
            "nothing is on the pool and no repair is owed, so the entry goes"
        );
    }

    #[test]
    fn the_entry_survives_while_other_objects_remain() {
        let pool = ThreadpoolPool::new().expect("create pool");
        let key = pool.as_raw() as usize;
        let (first, _entry) = entry_for(&pool);
        let (second, _) = entry_for(&pool);
        drop(first);
        assert!(
            crate::heal::entries().iter().any(|e| e.key() == key),
            "one object remains, so the entry must remain"
        );
        drop(second);
        assert!(!crate::heal::entries().iter().any(|e| e.key() == key));
    }

    #[test]
    fn an_entry_owing_a_repair_outlives_its_last_object() {
        // The retention rule, and the reason it is uniform across pool kinds:
        // retiring an entry that still owes a repair would drop the repair at
        // exactly the moment it is needed.
        let pool = ThreadpoolPool::new().expect("create pool");
        let key = pool.as_raw() as usize;
        let (work, entry) = entry_for(&pool);
        entry.stamp_cancelled(1);
        drop(work);
        assert!(
            crate::heal::entries().iter().any(|e| e.key() == key),
            "a repair is owed, so the entry is retained past its last object"
        );
        // A repair dispatching after the cancellation is what discharges it --
        // there is no clear to call. That is what makes the entry retirable,
        // which is what the self-heal timer relies on.
        entry.stamp_started(2);
        crate::heal::retire_idle();
        assert!(!crate::heal::entries().iter().any(|e| e.key() == key));
    }

    /// The defect `M-T9.2` exists for, stated as a sequence.
    ///
    /// The flag this replaces was set by `compare_exchange(0, at)` -- a no-op
    /// while a mark stood -- and cleared by an unconditional store. A
    /// cancellation arriving between a tick's read and its clear therefore
    /// landed nowhere: the exchange failed against the first stamp, and the
    /// clear then erased both. The repair that tick submitted happened strictly
    /// before that second cancellation, so it could not answer it, and the pool
    /// was left wedged with nothing scheduled.
    #[test]
    fn a_cancellation_during_a_tick_is_not_lost() {
        let pool = ThreadpoolPool::new().expect("create pool");
        let (work, entry) = entry_for(&pool);

        entry.stamp_cancelled(10);
        assert!(entry.unhealed(), "the first cancellation stands");

        // What a tick does: hands a repair to the pool. No clear.
        entry.stamp_submitted(11);

        // The cancellation the old scheme dropped on the floor, arriving while
        // that repair is still with the pool.
        entry.stamp_cancelled(20);
        assert_eq!(
            entry.last_cancelled(),
            20,
            "the later cancellation must move the stamp forward; the \
             compare-exchange this replaces would have left 10 here and the \
             tick's clear would then have erased even that"
        );

        // The repair submitted at 11 now runs. It answers the cancellation at
        // 10 and cannot answer the one at 20, because it was handed over first.
        entry.stamp_started(12);
        assert_eq!(entry.last_started(), 12);
        assert!(
            entry.unhealed(),
            "a repair dispatched at 12 cannot answer a cancellation at 20, so the pool is still \
             unhealed and the next tick must submit again"
        );
        assert!(
            !entry.repair_in_flight(),
            "the submitted repair has been given back, so a fresh one may be sent"
        );

        // And a repair that does follow it settles the matter.
        entry.stamp_submitted(21);
        entry.stamp_started(22);
        assert!(
            !entry.unhealed(),
            "a dispatch after the last cancellation heals it"
        );
        drop(work);
    }

    #[test]
    fn a_dispatch_after_the_cancellation_is_what_counts() {
        let pool = ThreadpoolPool::new().expect("create pool");
        let (work, entry) = entry_for(&pool);
        entry.stamp_started(5);
        entry.stamp_cancelled(10);
        assert!(
            entry.unhealed(),
            "a dispatch before the cancellation is no evidence the pool is live"
        );
        entry.stamp_started(11);
        assert!(!entry.unhealed());
        drop(work);
    }

    /// Equal stamps read as unhealed, which costs a redundant repair rather
    /// than risking a missed one.
    ///
    /// `QueryInterruptTime` has system-tick resolution -- about 15.6 ms -- so a
    /// cancellation and a dispatch inside one tick are indistinguishable by
    /// these values. Nothing here can say which came first.
    #[test]
    fn a_dispatch_in_the_same_tick_as_the_cancellation_does_not_heal_it() {
        let pool = ThreadpoolPool::new().expect("create pool");
        let (work, entry) = entry_for(&pool);
        entry.stamp_started(10);
        entry.stamp_cancelled(10);
        assert!(
            entry.unhealed(),
            "same-tick stamps cannot be ordered, so this must err toward repairing"
        );
        drop(work);
    }

    /// A repair the pool still holds is not a reason to send another.
    #[test]
    fn a_repair_in_flight_blocks_a_second_submission() {
        let pool = ThreadpoolPool::new().expect("create pool");
        let (work, entry) = entry_for(&pool);
        entry.stamp_cancelled(10);
        entry.stamp_submitted(11);
        assert!(entry.repair_in_flight());
        entry.stamp_started(12);
        assert!(
            !entry.repair_in_flight(),
            "the pool gave the repair back, so nothing is outstanding"
        );
        drop(work);
    }

    #[test]
    fn two_pools_get_two_entries() {
        let first = ThreadpoolPool::new().expect("create pool");
        let second = ThreadpoolPool::new().expect("create pool");
        let (_w1, e1) = entry_for(&first);
        let (_w2, e2) = entry_for(&second);
        assert_ne!(e1.key(), e2.key());
        assert_ne!(
            e1.repair_work(),
            e2.repair_work(),
            "each pool needs its own repair object: a repair is a submit to one pool"
        );
    }

    #[test]
    fn objects_on_the_default_pool_share_one_entry() {
        let a = ThreadpoolWork::new(|| {}, None).expect("create work");
        let b = ThreadpoolWork::new(|| {}, None).expect("create work");
        let count = crate::heal::entries()
            .iter()
            .filter(|e| e.key() == 0)
            .count();
        assert_eq!(count, 1, "the default pool is one pool and gets one entry");
        drop(a);
        drop(b);
    }

    // --- M-T6.2: every trampoline stamps its pool before calling the closure.

    use crate::wait::{ThreadpoolWait, WaitableHandle};
    use std::sync::atomic::Ordering;
    use std::time::{Duration, Instant};

    fn spin_until(label: &str, mut done: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !done() {
            assert!(Instant::now() < deadline, "timed out waiting for {label}");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    // --- M-T6.3: a cancellation marks its pool as owing a repair.
    //
    // The wider sabotage matrix for this -- a cancel that does not mark, a heal
    // that creates instead of submitting -- belongs to `M-T6.5`. These pin that
    // the two methods differ in the one way that is their whole point.

    #[test]
    fn try_cancel_pending_marks_its_pool() {
        let _gate = gate();
        let pool = ThreadpoolPool::new().expect("create pool");
        let mut env = CallbackEnviron::new();
        env.set_pool(&pool);
        let event = WaitableHandle::event(true, false).expect("create event");
        let wait = ThreadpoolWait::new(event, |_| {}, Some(&mut env)).expect("create wait");
        let entry = crate::heal::entries()
            .into_iter()
            .find(|e| e.key() == pool.as_raw() as usize)
            .expect("the wait registered its pool");
        assert!(!entry.unhealed(), "nothing owed before the call");

        wait.arm(None);
        wait.try_cancel_pending();
        assert!(
            entry.unhealed(),
            "a cancellation may have severed this pool, so it owes a repair"
        );
        entry.force_healed();
        // The cancel does not discharge the obligation -- it severs the pool's
        // watch without draining this object -- so the drain is still the
        // caller's to make, and under `fail-fast` the crate makes it.
        wait.stop_and_drain();
    }

    #[test]
    fn the_untracked_sibling_leaves_the_obligation_with_the_caller() {
        // The difference that justifies the `unsafe`: this one records nothing,
        // so the pool is the caller's to repair.
        let pool = ThreadpoolPool::new().expect("create pool");
        let mut env = CallbackEnviron::new();
        env.set_pool(&pool);
        let event = WaitableHandle::event(true, false).expect("create event");
        let wait = ThreadpoolWait::new(event, |_| {}, Some(&mut env)).expect("create wait");
        let entry = crate::heal::entries()
            .into_iter()
            .find(|e| e.key() == pool.as_raw() as usize)
            .expect("the wait registered its pool");

        wait.arm(None);
        // SAFETY: this test discharges the obligation by not depending on the
        // pool afterwards; the object is dropped immediately below.
        unsafe { wait.try_cancel_pending_no_heal_tracking() };
        assert!(
            !entry.unhealed(),
            "the untracked form must record nothing, or the `unsafe` is a lie"
        );
        wait.stop_and_drain();
    }

    #[test]
    fn a_cancelling_group_release_marks_a_wait_members_pool() {
        let _gate = gate();
        use crate::cleanup_group::CleanupGroup;

        let pool = ThreadpoolPool::new().expect("create pool");
        let mut env = CallbackEnviron::new();
        env.set_pool(&pool);
        let mut group = CleanupGroup::new().expect("create group");
        let member = group
            .create_wait(
                WaitableHandle::event(true, false).expect("create event"),
                |_| {},
                Some(&env),
            )
            .expect("create wait member");
        member.arm(None);
        let entry = crate::heal::entries()
            .into_iter()
            .find(|e| e.key() == pool.as_raw() as usize)
            .expect("the member registered its pool");
        assert!(!entry.unhealed());

        group.close_members_cancelling();
        assert!(
            entry.unhealed(),
            "a cancelling release passes the cancel to each member, so a wait \
             among them owes its pool a repair"
        );
        entry.force_healed();
    }

    #[test]
    fn a_draining_group_release_marks_nothing() {
        use crate::cleanup_group::CleanupGroup;

        let pool = ThreadpoolPool::new().expect("create pool");
        let mut env = CallbackEnviron::new();
        env.set_pool(&pool);
        let mut group = CleanupGroup::new().expect("create group");
        let member = group
            .create_wait(
                WaitableHandle::event(true, false).expect("create event"),
                |_| {},
                Some(&env),
            )
            .expect("create wait member");
        member.arm(None);
        let entry = crate::heal::entries()
            .into_iter()
            .find(|e| e.key() == pool.as_raw() as usize)
            .expect("the member registered its pool");

        group.close_members();
        assert!(
            !entry.unhealed(),
            "a draining release leaves nothing to remove, so nothing is owed"
        );
    }

    // --- M-T6.4: the healer actually repairs.

    #[test]
    fn the_healer_submits_a_repair_for_a_cancelled_pool() {
        // End to end: cancel, then wait for the healer's own timer to run the
        // repair work item. The repair's callback is this crate's, not the
        // test's, so what is observed is the submission arriving -- which is the
        // action measured to release the stall.
        let pool = ThreadpoolPool::new().expect("create pool");
        let mut env = CallbackEnviron::new();
        env.set_pool(&pool);
        let event = WaitableHandle::event(true, false).expect("create event");
        let wait = ThreadpoolWait::new(event, |_| {}, Some(&mut env)).expect("create wait");
        let entry = crate::heal::entries()
            .into_iter()
            .find(|e| e.key() == pool.as_raw() as usize)
            .expect("the wait registered its pool");

        // Gated only across the mark and the assertion about it, then released
        // so the healer can actually tick. Holding it through the wait below
        // would block the very timer this test is waiting for, which is how the
        // first version of this gate turned a passing test into a hang.
        {
            let _gate = gate();
            assert_eq!(entry.repairs_run(), 0, "nothing repaired on this pool yet");
            wait.arm(None);
            wait.try_cancel_pending();
            assert!(entry.unhealed(), "the cancel marks the pool");
        }

        // Waiting on *this entry's own* repair object having been dispatched,
        // which pins two things weaker assertions cannot.
        //
        // Not the owed mark: the tick clears that whether it submitted a repair
        // or decided to skip one, so a test waiting on it passes with the
        // submission deleted -- the whole behaviour under test.
        //
        // Not a process-wide count either: the registry is shared and these run
        // as threads in one process, so another test's repair could satisfy it.
        // This counter is reached through this entry's work object's own
        // context, so a tick that *created* a fresh object and submitted that
        // instead would leave it at zero -- which is the measured claim that
        // creating does not release a stall and only submitting does.
        spin_until(
            "this pool's pre-created repair item to be dispatched",
            || entry.repairs_run() > 0,
        );
        spin_until("the healer to discharge the mark", || !entry.unhealed());
        wait.stop_and_drain();
    }

    #[test]
    fn a_tick_leaves_a_pool_its_own_repair_has_answered() {
        let _gate = gate();
        // The coalescing rule, driven directly rather than through the timer:
        // a dispatch after the cancellation is evidence the pool is live, so no
        // repair is submitted. Asserted through the mark being cleared without
        // the work object having been submitted -- the pool is this test's and
        // nothing else can touch it.
        let pool = ThreadpoolPool::new().expect("create pool");
        let mut env = CallbackEnviron::new();
        env.set_pool(&pool);
        let work = ThreadpoolWork::new(|| {}, Some(&mut env)).expect("create work");
        let entry = crate::heal::entries()
            .into_iter()
            .find(|e| e.key() == pool.as_raw() as usize)
            .expect("the work registered its pool");

        entry.stamp_cancelled(10);
        entry.stamp_started(11);
        assert!(
            !entry.unhealed(),
            "a dispatch after the cancellation is what the skip rests on"
        );
        crate::heal::tick_inner();
        assert_eq!(
            entry.repairs_run(),
            0,
            "the pool dispatched after the cancellation, so no repair is owed \
             to it and none must be submitted -- asserted on the repair having \
             run rather than on the mark, which the tick clears either way"
        );
        assert!(
            !entry.unhealed(),
            "the tick leaves a healed pool healed: there is no mark to clear, so \
             it simply has nothing to do on this entry"
        );
        drop(work);
    }

    #[test]
    fn a_tick_leaves_a_pool_alone_when_nothing_is_owed() {
        let _gate = gate();
        let pool = ThreadpoolPool::new().expect("create pool");
        let mut env = CallbackEnviron::new();
        env.set_pool(&pool);
        let work = ThreadpoolWork::new(|| {}, Some(&mut env)).expect("create work");
        let entry = crate::heal::entries()
            .into_iter()
            .find(|e| e.key() == pool.as_raw() as usize)
            .expect("the work registered its pool");
        crate::heal::tick_inner();
        assert!(!entry.unhealed());
        drop(work);
    }

    /// Hold every repair callback inside its dispatch for as long as this lives.
    ///
    /// Process-wide, so it takes the tick gate: another test's repair running
    /// concurrently would also be held, and the entered flag is shared.
    struct HeldRepairCallback {
        key: usize,
        #[allow(dead_code)]
        gate: std::sync::MutexGuard<'static, ()>,
    }

    impl HeldRepairCallback {
        fn for_pool(key: usize, ms: u64) -> Self {
            let gate = gate();
            crate::heal::REPAIR_CALLBACK_INSIDE.store(0, Ordering::SeqCst);
            crate::heal::HOLD_REPAIR_CALLBACK_MS.store(ms, Ordering::SeqCst);
            crate::heal::HOLD_REPAIR_CALLBACK_FOR.store(key, Ordering::SeqCst);
            Self { key, gate }
        }

        fn wait_until_inside(&self) {
            spin_until("this pool's repair callback to enter its dispatch", || {
                crate::heal::REPAIR_CALLBACK_INSIDE.load(Ordering::SeqCst) == self.key
            });
        }
    }

    impl Drop for HeldRepairCallback {
        fn drop(&mut self) {
            crate::heal::HOLD_REPAIR_CALLBACK_FOR.store(0, Ordering::SeqCst);
            crate::heal::HOLD_REPAIR_CALLBACK_MS.store(0, Ordering::SeqCst);
            crate::heal::REPAIR_CALLBACK_INSIDE.store(0, Ordering::SeqCst);
        }
    }

    /// Retiring an entry whose repair callback is still running waits for it.
    ///
    /// `M-T9.1`. `RepairWork::drop` drains the work object before closing it,
    /// and this is the window that makes the drain necessary: the callback
    /// stamps `last_started` on entry, which is exactly what makes the entry
    /// retirable, and it then goes on to write to the entry again. A retirement
    /// landing between those two writes drops the last `Arc`, and without the
    /// drain `CloseThreadpoolWork` would return immediately -- it frees the
    /// work object asynchronously rather than waiting -- leaving the callback
    /// writing into an allocation the `Arc` has released.
    ///
    /// **Asserted on the drop blocking, not on a fault.** A use-after-free
    /// detected by a crash is a crash-caught result, which this repository
    /// treats as uncovered: it depends on allocator behaviour and reports the
    /// same way whether or not anything noticed. Measuring that the retirement
    /// waited for the callback states the property directly and fails cleanly,
    /// by name, when the drain is removed.
    ///
    /// Nothing here can land in the window by timing -- it is a few
    /// instructions wide -- so the callback is held inside it.
    #[test]
    fn retiring_an_entry_waits_for_a_repair_callback_already_running() {
        /// Long enough that the measurement cannot be mistaken for scheduler
        /// noise, short enough to stay instant.
        const HOLD: Duration = Duration::from_millis(400);
        /// What the drop must exceed to count as having waited. Below the hold
        /// so a slow start to the callback cannot fail an honest run.
        const WAITED: Duration = Duration::from_millis(200);

        let pool = ThreadpoolPool::new().expect("create pool");
        let key = pool.as_raw() as usize;
        let held = HeldRepairCallback::for_pool(key, HOLD.as_millis() as u64);
        let mut env = CallbackEnviron::new();
        env.set_pool(&pool);
        let work = ThreadpoolWork::new(|| {}, Some(&mut env)).expect("create work");
        let entry = crate::heal::entries()
            .into_iter()
            .find(|e| e.key() == key)
            .expect("the work registered its pool");

        // Unhealed, so the tick below submits a repair.
        //
        // Stamped `1` rather than `now()`, and that is not cosmetic.
        // `QueryInterruptTime` has about 15.6 ms resolution, and `unhealed`
        // counts equal stamps as unhealed on purpose -- so a cancellation and
        // the dispatch that answers it landing in one tick leaves the entry
        // unhealed, hence not retirable, and `retire_idle` below would do
        // nothing at all. Measured: with `now()` here this failed 3 runs in 20
        // at 32 test threads, reporting a 4 us retirement because there was
        // nothing to retire. Any real interrupt time is far above 1, so the
        // callback's stamp is unambiguously later.
        entry.stamp_cancelled(1);
        // Nothing of the caller's is left on the pool, so the entry is held
        // only by the registry and by this test.
        drop(work);

        crate::heal::tick_inner();
        assert!(
            entry.repairs_run() == 0,
            "the callback must still be inside its dispatch, not finished"
        );

        // Dropped before the retirement, so the registry's is the last claim
        // and `retire_idle` below performs the final drop.
        drop(entry);
        held.wait_until_inside();

        let started = Instant::now();
        crate::heal::retire_idle();
        let blocked_for = started.elapsed();

        assert!(
            blocked_for >= WAITED,
            "retiring the entry returned in {blocked_for:?} while its repair callback was still \
             running, so the close did not drain and the callback's remaining write lands in an \
             allocation the last `Arc` has released"
        );
        assert!(
            !crate::heal::entries().iter().any(|e| e.key() == key),
            "the entry was retirable, so it must be gone -- otherwise this measured some other \
             entry's drain"
        );
    }

    /// Force this pool's repair allocation to fail for as long as this lives.
    ///
    /// Keyed to one pool rather than switched on globally, so a test using it
    /// cannot fail a registration belonging to a test running beside it.
    struct ForcedRepairFailure;

    impl ForcedRepairFailure {
        fn for_pool(key: usize) -> Self {
            crate::heal::FORCE_REPAIR_FAILURE_FOR.store(key, Ordering::SeqCst);
            Self
        }
    }

    impl Drop for ForcedRepairFailure {
        fn drop(&mut self) {
            crate::heal::FORCE_REPAIR_FAILURE_FOR.store(0, Ordering::SeqCst);
        }
    }

    /// An object whose pool could not be registered still marks that pool when
    /// the allocation succeeds at cancel time.
    ///
    /// This is the case that used to be lost outright: the registration cached
    /// its miss and discarded the key, so the cancellation could neither find
    /// an entry nor make one, on a pool that was perfectly repairable.
    #[test]
    fn a_cancellation_registers_a_pool_whose_first_registration_failed() {
        let pool = ThreadpoolPool::new().expect("create pool");
        let key = pool.as_raw() as usize;

        // The registration an object got while the allocation was failing.
        let registration = {
            let _forced = ForcedRepairFailure::for_pool(key);
            let registration = crate::heal::register(key);
            assert!(
                crate::heal::entries().iter().all(|e| e.key() != key),
                "the forced failure must leave the pool unregistered, or this \
                 test is not exercising the path it names"
            );
            registration
        };

        // Driven at this level rather than through `ThreadpoolWait::try_cancel_pending`
        // on purpose. That method runs the untracked fail-fast, which panics
        // under `fail-fast` -- and `--all-features` turns it on -- so a
        // regression here would unwind through an armed wait's `Drop`, panic a
        // second time, and abort. The harness scores an abort as a failure
        // either way, which is precisely the weak kind of red this avoids:
        // sabotaging the retry makes these two assertions fail by name.
        let _gate = gate();
        assert!(
            registration.owe_repair(),
            "the retry must register the pool it could not register before"
        );
        let entry = crate::heal::entries()
            .into_iter()
            .find(|e| e.key() == key)
            .expect("the retry created the entry");
        assert!(
            entry.unhealed(),
            "the recovered entry must be marked, or the repair never runs"
        );
        entry.force_healed();
    }

    /// With every allocation failing, the cancellation reports the pool
    /// untracked rather than claiming a repair it did not arrange.
    ///
    /// The other direction of the same guard: the test above proves it does not
    /// report untracked when it recovered, this one that it does when it could
    /// not.
    #[test]
    fn a_cancellation_that_cannot_register_reports_the_pool_untracked() {
        let pool = ThreadpoolPool::new().expect("create pool");
        let key = pool.as_raw() as usize;
        let mut env = CallbackEnviron::new();
        env.set_pool(&pool);

        let _forced = ForcedRepairFailure::for_pool(key);
        // The registration an object would hold, taken directly: this is the
        // unit the report comes from, and reaching it through a live wait would
        // add a teardown that says nothing about the question.
        let registration = crate::heal::register(key);
        let tracked = registration.owe_repair();
        assert!(
            !tracked,
            "every allocation failed, so the cancellation cannot claim the pool is tracked"
        );
        assert!(
            crate::heal::entries().iter().all(|e| e.key() != key),
            "nothing could be allocated, so no entry should exist"
        );
        drop(env);
    }

    /// The ordinary case reports tracked, so the assertion above is not simply
    /// reading a value that is always `false`.
    #[test]
    fn a_cancellation_on_a_registered_pool_reports_the_pool_tracked() {
        let pool = ThreadpoolPool::new().expect("create pool");
        let key = pool.as_raw() as usize;
        let _gate = gate();

        let registration = crate::heal::register(key);
        assert!(
            registration.owe_repair(),
            "the pool registered, so the cancellation tracks it"
        );
        let entry = crate::heal::entries()
            .into_iter()
            .find(|e| e.key() == key)
            .expect("the registration created an entry");
        entry.force_healed();
    }
}
