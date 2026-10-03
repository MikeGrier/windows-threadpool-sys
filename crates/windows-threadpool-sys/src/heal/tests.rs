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
    /// **The second reason, which `M-T9.2` created and which is sharper than the
    /// first: a test driving an entry with small synthetic stamps is reading
    /// fields a concurrent tick writes with the real clock.** A tick stamps
    /// `last_submitted` with `now()`, and the repair it submits stamps
    /// `last_started` the same way -- both astronomically larger than the `1`,
    /// `10`, `12` these tests use -- so one interleaved tick inverts
    /// `repair_in_flight`, `unhealed`, or whether an entry is retirable,
    /// whichever the test went on to assert. The old flag could not do this: it
    /// held no clock value, so a tick could only set or clear it.
    ///
    /// Measured, not supposed: `an_entry_owing_a_repair_outlives_its_last_object`
    /// failed on run 18 of 25 at 32 test threads, on its final assertion, with a
    /// repair submitted between its `stamp_cancelled(1)` and `stamp_started(2)`
    /// leaving `last_submitted` at a real timestamp and the entry therefore
    /// unretirable. Five tests in this module take synthetic stamps and all five
    /// now hold this gate; fixing only the one that happened to fail would have
    /// left the same defect at four sites.
    ///
    /// A test that wants to tick calls `crate::heal::tick_inner` while holding
    /// this; `crate::heal::tick` would deadlock on the gate it already has.
    fn gate() -> std::sync::MutexGuard<'static, ()> {
        crate::heal::TICK_GATE
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    /// Model a repair beginning to run, the way the trampoline does.
    ///
    /// Both halves, because a dispatch is both: the stamp orders it against a
    /// cancellation, and the count says one fewer repair is queued. Two tests
    /// used to call `stamp_started` alone and assert the entry was no longer in
    /// flight, which passed only because `repair_in_flight` was then a
    /// comparison of those stamps -- the very equivalence `M-T11.2` removed,
    /// because it reads false with a second repair still queued. One helper, so
    /// a test cannot model half a dispatch again.
    fn dispatch_repair(entry: &Arc<PoolEntry>, at: u64) {
        entry.stamp_started(at);
        entry.note_repair_started();
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
        let _gate = gate();
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
        let _gate = gate();
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
        dispatch_repair(&entry, 12);
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
        dispatch_repair(&entry, 22);
        assert!(
            !entry.unhealed(),
            "a dispatch after the last cancellation heals it"
        );
        drop(work);
    }

    #[test]
    fn a_dispatch_after_the_cancellation_is_what_counts() {
        let _gate = gate();
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
        let _gate = gate();
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
        let _gate = gate();
        let pool = ThreadpoolPool::new().expect("create pool");
        let (work, entry) = entry_for(&pool);
        entry.stamp_cancelled(10);
        entry.stamp_submitted(11);
        assert!(entry.repair_in_flight());
        dispatch_repair(&entry, 12);
        assert!(
            !entry.repair_in_flight(),
            "the pool gave the repair back, so nothing is outstanding"
        );
        drop(work);
    }

    /// Two repairs outstanding: the first starting does not clear the second.
    ///
    /// The defect `M-T11.2` fixed, stated as the sequence that produced it. The
    /// overdue retry hands over a second repair before the first has started,
    /// and `repair_in_flight` used to be `last_submitted > last_started` -- so
    /// the first starting, at a time later than the second submission, made the
    /// predicate false while the second was still queued. `is_retirable` then
    /// permitted retirement, and the drain in `PoolEntry::drop` would wait for
    /// a callback on a pool suspected of not dispatching, on the healer's only
    /// thread, stopping self-heal for every pool in the process.
    ///
    /// The timestamps here are the ones that make the old predicate wrong:
    /// submitted at 11 and 12, started at 13, which is greater than both.
    #[test]
    fn a_retry_leaves_two_repairs_outstanding_until_both_have_started() {
        let _gate = gate();
        let pool = ThreadpoolPool::new().expect("create pool");
        let (work, entry) = entry_for(&pool);

        entry.stamp_cancelled(10);
        entry.stamp_submitted(11);
        entry.stamp_submitted(12);

        dispatch_repair(&entry, 13);
        assert!(
            entry.repair_in_flight(),
            "one of two repairs has started; the other is still queued, so the pool still holds \
             one. The stamp comparison this replaces read false here, because 12 is not greater \
             than 13"
        );
        assert!(
            !crate::heal::is_retirable(&entry),
            "and an entry with a repair still queued must not be retirable: dropping it drains, \
             and that drain cannot return until a pool we suspect of not dispatching dispatches"
        );

        dispatch_repair(&entry, 14);
        assert!(
            !entry.repair_in_flight(),
            "both repairs have now started, so nothing is outstanding"
        );
        drop(work);
    }

    /// A cancellation published late cannot undo a newer one.
    ///
    /// The other half of `M-T11.2`. Reading a monotonic clock does not make the
    /// *publication* monotonic: any thread may cancel, so a thread that reads
    /// its timestamp and is then preempted can store it after a later
    /// cancellation has already stored a greater one. With a plain store the
    /// older value won and `unhealed` read healthy with a cancellation
    /// unanswered -- the same loss the replaced flag suffered, re-entering
    /// through the publication rather than the compare-exchange.
    ///
    /// Asserted through `unhealed` as well as through the stamp, because the
    /// stamp is only the mechanism; the consequence is the pool being called
    /// healthy.
    #[test]
    fn a_cancellation_published_out_of_order_does_not_lower_the_stamp() {
        let _gate = gate();
        let pool = ThreadpoolPool::new().expect("create pool");
        let (work, entry) = entry_for(&pool);

        // The interleaving: a repair dispatches at 20, a cancellation at 30 is
        // published, and only then does a cancellation that read 10 land.
        dispatch_repair(&entry, 20);
        entry.stamp_cancelled(30);
        entry.stamp_cancelled(10);

        assert_eq!(
            entry.last_cancelled(),
            30,
            "the newest cancellation must stand; a plain store would leave 10 here"
        );
        assert!(
            entry.unhealed(),
            "and the pool must still read unhealed -- with 10 stored, 10 >= 20 is false and a \
             cancellation nothing has answered would be reported as healthy"
        );
        drop(work);
    }

    /// A cancellation whose healer will not start submits its repair inline.
    ///
    /// The gap a review found: `owe_repair` stamped the entry, called
    /// `ensure_running`, and reported the cancellation as tracked **whatever
    /// that returned**. When the healer could not be started there was then a
    /// pool marked unhealed with nothing scheduled to visit it, while
    /// `try_cancel_pending`'s claim that this crate repairs the pool afterwards
    /// silently did not hold and the untracked fail-fast did not fire either.
    ///
    /// A later cancellation anywhere in the process retries the start, which is
    /// `M-T10.22`. What that does not cover is a process where no later
    /// cancellation arrives -- so the repair is now submitted on the spot.
    ///
    /// **The gate is what makes the observation unambiguous.** It stops the
    /// healer's own tick, so a submission seen here can only be the inline one;
    /// without it, a real tick could supply the same evidence and the test
    /// would pass whether or not the fallback existed.
    #[test]
    fn a_cancellation_submits_its_repair_when_the_healer_will_not_start() {
        let _gate = gate();
        let pool = ThreadpoolPool::new().expect("create pool");
        let (work, entry) = entry_for(&pool);
        let key = entry.key();

        // The accepting direction first: with a healer available, the
        // cancellation path schedules rather than submits, so nothing is handed
        // over while the gate holds every tick off.
        let registration = crate::heal::register(key);
        assert!(
            registration.owe_repair(),
            "a registered pool's cancellation is tracked"
        );
        assert_eq!(
            entry.last_submitted(),
            0,
            "with a healer running the repair is left to a tick, and the gate is holding \
             every tick off -- a submission here would mean the fallback fires when it \
             should not, which would cost the coalescing the timer exists to provide"
        );

        // And the direction the fallback is for.
        crate::heal::FORCE_HEALER_START_FAILURE.store(true, Ordering::SeqCst);
        let registration = crate::heal::register(key);
        let tracked = registration.owe_repair();
        crate::heal::FORCE_HEALER_START_FAILURE.store(false, Ordering::SeqCst);

        assert!(
            tracked,
            "the cancellation is still tracked: its repair has been handed over rather than \
             scheduled, which is a stronger answer than scheduling, not a weaker one"
        );
        assert!(
            entry.last_submitted() > 0,
            "no healer will ever tick this entry, so the repair must have been submitted here. \
             Reporting tracked without submitting leaves a pool marked unhealed with nothing \
             arranged to repair it"
        );
        assert!(
            entry.repair_in_flight(),
            "and it must be counted as outstanding, or retirement could drop the entry while \
             that repair is still queued"
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

    /// Assert that a tick handed this entry no repair.
    ///
    /// **Asserted on the submission, never on the repair having run.**
    /// `repairs_run` counts repairs that have *dispatched*, which is
    /// asynchronous: on a fresh private pool with no thread started yet, a
    /// repair submitted by the tick is still 0 on the next line, so a test
    /// written against it passes whether or not the submission happened. Both
    /// callers below were written that way, and deleting the skip they exist to
    /// cover left the whole lib suite green.
    ///
    /// `stamp_submitted` raises `outstanding` and stores `last_submitted`
    /// immediately before `SubmitThreadpoolWork`, so both are already written
    /// by the time `tick_inner` returns. They are the synchronous evidence.
    fn assert_no_repair_was_handed_over(entry: &crate::heal::PoolEntry, because: &str) {
        assert_eq!(
            entry.last_submitted(),
            0,
            "{because}, so the tick must not submit a repair -- `last_submitted` \
             is stamped before the submit it accompanies, so a non-zero value \
             here is a submission that happened"
        );
        assert!(
            !entry.repair_in_flight(),
            "{because}, so nothing must be outstanding on this entry"
        );
    }

    #[test]
    fn a_tick_leaves_a_pool_its_own_repair_has_answered() {
        let _gate = gate();
        // The coalescing rule, driven directly rather than through the timer:
        // a dispatch after the cancellation is evidence the pool is live, so no
        // repair is submitted. The pool is this test's and nothing else can
        // touch it, so the entry's counters are this tick's doing alone.
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
        assert_no_repair_was_handed_over(&entry, "the pool dispatched after the cancellation");
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
        // Nothing has ever cancelled on this pool, so `unhealed` is false for a
        // reason the tick cannot change -- which is why this asserts on the
        // submission instead. `last_cancelled` is 0, so `!unhealed()` holds
        // whatever the tick does, and a version of this test that checked only
        // that could not fail.
        let pool = ThreadpoolPool::new().expect("create pool");
        let mut env = CallbackEnviron::new();
        env.set_pool(&pool);
        let work = ThreadpoolWork::new(|| {}, Some(&mut env)).expect("create work");
        let entry = crate::heal::entries()
            .into_iter()
            .find(|e| e.key() == pool.as_raw() as usize)
            .expect("the work registered its pool");
        crate::heal::tick_inner();
        assert_no_repair_was_handed_over(&entry, "nothing has cancelled on this pool");
        assert!(
            !entry.unhealed(),
            "and an entry that was never marked stays unmarked"
        );
        drop(work);
    }

    /// Where a stalled pool's blocking callback reports from, and waits.
    #[derive(Default)]
    struct OccupancyState {
        /// Set once the pool's only thread is inside the blocking callback.
        inside: bool,
        /// Set to let that callback return, so the work object can be drained.
        released: bool,
    }

    /// The handshake between a stalled pool's callback and the test driving it.
    #[derive(Default)]
    struct Occupancy {
        state: std::sync::Mutex<OccupancyState>,
        changed: std::sync::Condvar,
    }

    impl Occupancy {
        fn lock(&self) -> std::sync::MutexGuard<'_, OccupancyState> {
            self.state
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
        }

        /// Wait for `done`, or give up after `limit`. False means it timed out.
        fn wait_for(&self, limit: Duration, done: impl Fn(&OccupancyState) -> bool) -> bool {
            let mut state = self.lock();
            while !done(&state) {
                let (next, timeout) = self
                    .changed
                    .wait_timeout(state, limit)
                    .unwrap_or_else(|poison| poison.into_inner());
                if timeout.timed_out() {
                    return done(&next);
                }
                state = next;
            }
            true
        }
    }

    /// A pool that cannot dispatch anything, because its one thread is busy.
    ///
    /// **The difference between this and holding the repair callback is the
    /// reason it exists.** `repair_trampoline` stamps `last_started` and lowers
    /// the outstanding count as its very first acts, *before* the test hold --
    /// deliberately, so an entry can be retired while its callback is still
    /// running. A held callback has therefore already made `repair_in_flight`
    /// false, and an entry whose repair has run cannot be put back into the
    /// overdue state by hand: that needs something outstanding *and* the most
    /// recent submission lagging the clock by five seconds, so re-entering it
    /// means waiting out the real threshold or submitting afresh.
    ///
    /// Occupying the pool's only thread keeps the repair from ever starting, so
    /// nothing is ever taken off the count and the state stays reachable. It is also
    /// the state the feature is about rather than a simulation of it: a pool
    /// that is not dispatching.
    struct StalledPool {
        /// `Option` so [`Drop`] can release the callback and drain the work
        /// *before* the pool it runs on is closed. Field order alone would drop
        /// it first but would not let the callback return, so the drain would
        /// block for the callback's full timeout.
        blocker: Option<ThreadpoolWork>,
        pool: ThreadpoolPool,
        occupancy: Arc<Occupancy>,
    }

    impl StalledPool {
        /// `None` if the pool could not be built or its thread not occupied.
        fn new() -> Option<Self> {
            let pool = ThreadpoolPool::new().ok()?;
            // One thread, so one blocking callback is the whole pool.
            pool.set_max_threads(1).ok()?;

            let occupancy = Arc::new(Occupancy::default());
            let in_callback = Arc::clone(&occupancy);
            let mut env = CallbackEnviron::new();
            env.set_pool(&pool);
            let blocker = ThreadpoolWork::new(
                move || {
                    in_callback.lock().inside = true;
                    in_callback.changed.notify_all();
                    // Bounded, so a leaked instance cannot park a pool thread
                    // for the rest of the test binary's life.
                    in_callback.wait_for(Duration::from_secs(120), |s| s.released);
                },
                Some(&mut env),
            )
            .ok()?;
            // The environment borrows the pool, and `CreateThreadpoolWork` has
            // already copied it, so release the borrow before the pool moves
            // into the returned value.
            drop(env);
            blocker.submit();

            // Submitting a repair before the thread is occupied would let it
            // run, which is precisely what this type exists to prevent.
            occupancy
                .wait_for(Duration::from_secs(30), |s| s.inside)
                .then_some(())?;

            Some(Self {
                blocker: Some(blocker),
                pool,
                occupancy,
            })
        }

        fn key(&self) -> usize {
            self.pool.as_raw() as usize
        }

        /// This pool's registry entry, which the blocking work item created.
        fn entry(&self) -> Arc<PoolEntry> {
            let key = self.key();
            crate::heal::entries()
                .into_iter()
                .find(|e| e.key() == key)
                .expect("the blocking work registered its pool")
        }
    }

    impl Drop for StalledPool {
        fn drop(&mut self) {
            self.occupancy.lock().released = true;
            self.occupancy.changed.notify_all();
            if let Some(blocker) = self.blocker.take() {
                // The drain this type owes for having submitted. Released
                // first, or it would block until the callback's own timeout.
                blocker.stop_and_drain();
            }
        }
    }

    /// A repair the pool has not taken is re-submitted once, and reported.
    ///
    /// The `M-T9.2` deferral, decided 2026-10-02. A repair sitting with a pool
    /// unstarted is almost always "queued behind other work", which is why
    /// `repair_in_flight` suppresses a second submission at all -- but past a
    /// generous threshold that reading stops being the likely one, and the
    /// previous behaviour was to stay silent about it forever.
    ///
    /// Driven by backdating the submission rather than by waiting out the real
    /// five-second threshold, so the test is instant and exact.
    #[test]
    fn a_repair_the_pool_has_not_taken_is_resubmitted_and_reported() {
        let _gate = gate();
        let stalled = StalledPool::new().expect("occupy a pool's only thread");
        let entry = stalled.entry();

        backdate_an_overdue_repair(&entry);

        crate::heal::tick_inner();
        assert_eq!(
            entry.repair_overdue_count(),
            1,
            "the tick must notice a repair the pool has not taken"
        );
        let resubmitted = entry.last_submitted();
        assert!(
            resubmitted > 1,
            "and must re-submit it rather than only reporting -- the stamp moves to now"
        );

        // A second tick immediately after must do nothing: the re-submission is
        // fresh, so it is in flight but not overdue. Without that distinction
        // this would queue one repair per tick forever. The pool cannot have
        // started it, so this is that branch and not an entry that looks healed.
        assert!(entry.repair_in_flight());
        crate::heal::tick_inner();
        assert_eq!(
            entry.repair_overdue_count(),
            1,
            "a repair submitted moments ago is the ordinary case, not an overdue one"
        );
        assert_eq!(
            entry.last_submitted(),
            resubmitted,
            "and must not be submitted again, or every tick would queue one more"
        );
    }

    /// Set an entry up as unhealed with a repair handed over and long overdue.
    ///
    /// The two stamps the overdue path reads, at the dawn of the counter, so
    /// the threshold is already passed without waiting out the real five
    /// seconds. One helper rather than the same pair of stamps written at each
    /// site, because the two must agree for the entry to be in flight at all.
    fn backdate_an_overdue_repair(entry: &Arc<PoolEntry>) {
        entry.stamp_cancelled(1);
        entry.stamp_submitted(1);
        assert!(
            entry.repair_in_flight(),
            "the backdated stamps must leave a repair in flight, or the tick \
             under test takes the submit path instead of the overdue one"
        );
    }

    /// Without `fail-fast`, a pool that stays stuck is re-submitted to forever.
    ///
    /// The other half of the decision the test above covers: past the
    /// reattempt allowance this crate has nothing left to try, and the choice
    /// recorded for the feature-off build is to keep reporting and keep
    /// re-submitting rather than to give up and go quiet. A second episode is
    /// enough to show the loop does not stop, because nothing in the path
    /// counts episodes except the counter asserted here.
    ///
    /// Feature-gated because this is exactly the state the `fail-fast` build
    /// ends the process on -- see
    /// `the_fail_fast_build_ends_the_process_on_a_pool_that_will_not_be_repaired`.
    #[cfg(not(feature = "fail-fast"))]
    #[test]
    fn without_fail_fast_a_pool_that_will_not_be_repaired_is_retried_indefinitely() {
        let _gate = gate();
        let stalled = StalledPool::new().expect("occupy a pool's only thread");
        let entry = stalled.entry();

        for episode in 1..=3 {
            backdate_an_overdue_repair(&entry);
            crate::heal::tick_inner();
            assert_eq!(
                entry.repair_overdue_count(),
                episode,
                "episode {episode} must be counted: the retry is indefinite, so no \
                 episode is the last one"
            );
            assert!(
                entry.last_submitted() > 1,
                "and must re-submit on episode {episode} rather than only counting it"
            );
        }
    }

    /// With `fail-fast`, the same state ends the process.
    ///
    /// **What this establishes is the panic and its message, not the abort.**
    /// In the shipped path the panic escapes the healer's `extern "system"`
    /// timer trampoline, which Rust turns into an abort; this test does not
    /// traverse that path. The child calls `tick_inner` directly on a libtest
    /// thread, so the panic unwinds, libtest catches it, and the child exits
    /// 101. Every assertion below -- not `SURVIVED`, not `SETUP_FAILED`,
    /// non-zero, the message on stderr -- holds under that unwind, so none of
    /// them would tell an abort from a caught panic.
    ///
    /// It runs in a child anyway, because a panic on the test's own thread
    /// would fail the test rather than demonstrate anything, and the state has
    /// to be reached through `pub(crate)` stamps an integration test cannot
    /// see. `tests/callback_panic_aborts.rs` is where the abort itself is
    /// covered, by going through a real trampoline.
    ///
    /// Asserts the stderr message too, not only the exit status. A child that
    /// died silently would satisfy an exit-code-only assertion while telling an
    /// operator nothing, and the claim that the message survives an abort is
    /// one this crate makes in `fail_fast_if_unrepairable`'s own documentation.
    #[cfg(feature = "fail-fast")]
    #[test]
    fn the_fail_fast_build_ends_the_process_on_a_pool_that_will_not_be_repaired() {
        /// Names the child; absent in the parent.
        const CHILD_VAR: &str = "WTPS_UNREPAIRABLE_CHILD";
        /// The child's exit code if it survives the state that must end it.
        const SURVIVED: i32 = 9;
        /// The child's exit code when it fails before reaching that state. Any
        /// other nonzero code reads as the abort, so a setup failure has to be
        /// distinguishable from the thing under test.
        const SETUP_FAILED: i32 = 111;
        const NAME: &str = "heal::tests::on::\
                            the_fail_fast_build_ends_the_process_on_a_pool_that_will_not_be_repaired";

        if std::env::var(CHILD_VAR).is_ok() {
            let _gate = gate();
            let Some(stalled) = StalledPool::new() else {
                std::process::exit(SETUP_FAILED);
            };
            let entry = stalled.entry();

            // Two episodes: the allowance is one reattempt, so the first
            // re-submits and the second is the one with nothing left to try.
            for _ in 0..2 {
                backdate_an_overdue_repair(&entry);
                crate::heal::tick_inner();
            }

            std::process::exit(SURVIVED);
        }

        let exe = std::env::current_exe().expect("locate the test binary");
        let out = std::process::Command::new(exe)
            .env(CHILD_VAR, "1")
            .args(["--exact", NAME, "--test-threads", "1", "--nocapture"])
            .output()
            .expect("run the child");

        assert_ne!(
            out.status.code(),
            Some(SURVIVED),
            "the child reached the end of a state `fail-fast` is supposed to stop on"
        );
        assert_ne!(
            out.status.code(),
            Some(SETUP_FAILED),
            "the child failed during setup, so it never reached the state under test"
        );
        assert_ne!(
            out.status.code(),
            Some(0),
            "the child exited cleanly -- exit 0 also means `--exact {NAME}` matched \
             nothing, so check the test has not been renamed without updating `NAME`"
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("has not dispatched a repair"),
            "the child died without saying why; an abort that reports nothing leaves \
             an operator with an exit code and no cause. stderr was:\n{stderr}"
        );
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
    /// `M-T9.1`. `PoolEntry::drop` drains the work object before closing it,
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
