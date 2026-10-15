// Copyright (c) Mike Grier
//! The pools this crate is interacting with, and what each one is owed.
//!
//! Backs [Cancellation repairs the pool it may have
//! wedged](../../../DESIGN-NOTES.md#cancellation-self-heals). A cancellation can
//! leave a pool unable to make its first worker; submitting a work item to it is
//! the one action measured to release that stall every time. This is the record
//! of which pools exist, which are owed a repair, and the work object each
//! repair will use.
//!
//! # Why the repair object is made here and not when it is needed
//!
//! *Creating* a work object was measured not to release a stall -- only
//! *submitting* one is. So the object has to exist before the pool is wedged,
//! which means making it when the pool is first registered, on a path where
//! allocation is ordinary, rather than on the healing path where the pool may
//! already be stuck.
//!
//! # Cost when the feature is off
//!
//! Nothing. [`Registration`] is a zero-sized type with an empty constructor and
//! no `Drop`, so every call site compiles away rather than being conditioned
//! with `cfg` in five modules.

use crate::callback_env::CallbackEnviron;

/// Identifies a pool: its raw `PTP_POOL`, with zero meaning the process default.
///
/// A raw pointer value is enough because the registry only ever compares keys
/// and never dereferences one. The pool behind a non-zero key is kept alive for
/// as long as its entry exists by the repair work object bound to it, so a key
/// cannot come to name a different pool while the entry holding it is live.
pub(crate) type PoolKey = usize;

/// Which pool an object being created will run its callbacks on.
///
/// `None` and an environment with no pool set both mean the process default.
pub(crate) fn key_of(env: Option<&CallbackEnviron<'_>>) -> PoolKey {
    env.map_or(0, |env| env.as_inner().Pool as PoolKey)
}

// `PoolEntry`, `entries` and `retire_idle` are the surface the self-heal timer
// reads, and `M-T6.4` is what adds that timer. They are exported now because the
// entry's shape -- what a pool is owed, and who can ask -- is this item's
// deliverable rather than the timer's, so the crate's own tests are their only
// caller until that item lands.
#[cfg(feature = "self-heal")]
#[allow(unused_imports)]
pub(crate) use on::{PoolEntry, Registration, entries, now, register, retire_idle, tick};
// The module these name exists only with the feature on, so the re-export has
// to carry that condition too -- `#[cfg(test)]` alone resolves against a module
// that is not there in a `--no-default-features` test build.
#[cfg(all(test, feature = "self-heal"))]
pub(crate) use on::{
    FORCE_HEALER_START_FAILURE, ForcedRepairFailure, HOLD_REPAIR_CALLBACK_FOR,
    HOLD_REPAIR_CALLBACK_MS, REPAIR_CALLBACK_INSIDE, TICK_GATE, is_retirable, tick_inner,
};

#[cfg(not(feature = "self-heal"))]
pub(crate) use off::{Registration, register};

#[cfg(not(feature = "self-heal"))]
mod off {
    use super::PoolKey;

    /// A registration in a build with no registry: nothing, costing nothing.
    pub(crate) struct Registration;

    /// "Costing nothing" is the claim, so it is checked by the build.
    ///
    /// Five types carry this as a field and five trampolines call its methods,
    /// unconditionally -- the call sites have no `cfg` on them, which is the
    /// point of the type existing in both configurations. If it ever gained a
    /// byte, every one of those would start paying for a feature that is off.
    const _: () = assert!(
        size_of::<Registration>() == 0,
        "the feature-off registration must be zero-sized"
    );

    impl Registration {
        /// Records nothing. Without the feature the repair obligation belongs to
        /// the caller of `try_cancel_pending_no_heal_tracking`, not to us.
        ///
        /// Claims nothing: without the feature there is no registry to claim
        /// in, and no repair whose object could keep a pool alive.
        #[must_use]
        pub(crate) const fn reclaim(&self) -> Registration {
            Registration
        }

        /// Nothing is tracked, so nothing can be untracked, and the caller's
        /// fail-fast has nothing to report.
        ///
        /// Reports `true`: the question the value answers is "did this crate
        /// leave a pool unrepaired behind a safety claim it made", and in this
        /// build the crate makes no such claim, so there is nothing to report.
        /// Answering `false` would fire the untracked fail-fast on every
        /// cancellation in a configuration that never promised a repair.
        ///
        /// The feature-on twin has a second form, `owe_repair`, for the path
        /// that may retry its registration. There is deliberately no mirror of
        /// it here: with the feature off, `try_cancel_pending` does not exist
        /// and the group's marking pass is the only caller left, so a second
        /// method would be an unused shape kept only for symmetry.
        #[must_use]
        pub(crate) const fn owe_repair_claimed(&self) -> bool {
            true
        }
    }

    /// Records nothing, because without the feature there is no repair to owe.
    pub(crate) const fn register(_key: PoolKey) -> Registration {
        Registration
    }
}

#[cfg(feature = "self-heal")]
mod on {
    use super::PoolKey;
    use std::sync::atomic::{AtomicIsize, AtomicU32, AtomicU64, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex, OnceLock};
    use std::time::Duration;
    use win_time_sys::{Clock, InterruptClock};
    use windows_sys::Win32::Foundation::FALSE;
    use windows_sys::Win32::System::Threading::{
        CloseThreadpoolWork, CreateThreadpoolWork, PTP_CALLBACK_INSTANCE, PTP_WORK,
        SubmitThreadpoolWork, WaitForThreadpoolWorkCallbacks,
    };

    /// What this crate knows about one pool.
    ///
    /// The two stamps are compared against each other and never against a
    /// wall-clock, so the only requirement on their source is that it be
    /// monotonic and shared -- see `M-T6.2`, which picks it.
    pub(crate) struct PoolEntry {
        /// The work item a repair submits.
        ///
        /// Declaration order carries nothing here: this entry's own
        /// [`Drop`](PoolEntry::drop) drains the work object before any field is
        /// dropped, so the teardown is correct whatever order the fields are
        /// written in and whatever a later one comes to own. Two earlier
        /// revisions did rest on this being first, and the second of them was
        /// simply wrong about why.
        repair: RepairWork,
        key: PoolKey,
        /// How many live objects this crate has on the pool.
        objects: AtomicUsize,
        /// When a cancellation last left this pool possibly severed.
        ///
        /// One of two stamps that between them replace a stored "repair owed"
        /// flag, which was set with a compare-exchange deliberately keeping the
        /// earliest cancellation and cleared with an unconditional store. The
        /// two did not pair, so a cancellation landing between `tick`'s read
        /// and its clear was recorded nowhere -- the CAS failed against the old
        /// stamp and the clear then erased both.
        ///
        /// **Published with `fetch_max`, not a store, and that is load-bearing
        /// rather than tidiness.** Any thread may cancel, so there are
        /// concurrent writers, and reading a monotonic clock does not make the
        /// *publication* monotonic: a thread that reads 10 and is preempted
        /// past a dispatch at 20 and another cancellation at 30 would, with a
        /// plain store, write 10 last and leave [`unhealed`](Self::unhealed)
        /// reading healthy with a cancellation unanswered. That is the same
        /// lost cancellation the flag suffered, re-entering through the
        /// publication instead of through the compare-exchange. `fetch_max`
        /// keeps the newest, which is the direction that keeps a pool repaired.
        last_cancelled: AtomicU64,
        /// When this entry's own repair callback last ran.
        ///
        /// Written by the repair trampoline, so it is direct evidence that the
        /// pool dispatched *our* work item. That is strictly better evidence
        /// than the dispatch stamp it replaces, which depended on a user
        /// object's trampoline firing: a pool with no other activity was
        /// indistinguishable from a wedged one.
        ///
        /// Also `fetch_max`: a retry can leave two repairs outstanding, so two
        /// trampolines can run at once and publish out of order. Here the stale
        /// write would cost only a redundant repair rather than a missed one,
        /// but the two stamps are read against each other and a rule that holds
        /// for one of them is not a rule.
        last_started: AtomicU64,
        /// When a repair was last handed to the pool.
        ///
        /// Timing only -- how long the most recent attempt has been waiting,
        /// for [`repair_overdue`](Self::repair_overdue). Whether anything is
        /// outstanding is `outstanding`'s question, not this one.
        ///
        /// Published with `fetch_max`, as the other two stamps are.
        ///
        /// **This was a plain store, on the argument that submissions come only
        /// from a tick and ticks are serialised on the healer's single thread.**
        /// That stopped being true when `mark_and_arrange_repair` gained its
        /// inline fallback: if the healer cannot be started, *every cancelling
        /// thread* submits the repair itself, and cancellation is public API
        /// callable from anywhere. Two such threads can read the clock, be
        /// preempted, and store out of order, leaving an older value behind a
        /// newer one.
        ///
        /// The loss is not a lost repair but a lost *deadline*:
        /// [`repair_overdue`](Self::repair_overdue) measures `now -
        /// last_submitted`, so a stale-but-earlier value makes the entry look
        /// overdue sooner than it is. That retries early, and on a
        /// `fail-fast` build enough early retries end the process. A stamp read
        /// against a clock has to be published monotonically for the same
        /// reason the other two are.
        ///
        /// Moving it *backwards* is a test need, not a production one, so it
        /// has its own hook, `backdate_submitted` -- named rather than linked,
        /// because it is `cfg(test)` and a link to it does not resolve in a
        /// documentation build. A `fetch_max` would refuse a backdate silently
        /// while the test still passed, which is the trap that kept this a
        /// plain store.
        last_submitted: AtomicU64,
        /// Repairs handed to the pool that have not yet begun running.
        ///
        /// **The predicate `last_submitted > last_started` used to answer this,
        /// and it was sound only while at most one repair could be
        /// outstanding.** The overdue retry submits a second before the first
        /// has started, and then the first starting makes that comparison false
        /// while the second is still queued -- so an entry with a repair
        /// outstanding looked idle, [`is_retirable`] permitted retirement, and
        /// [`PoolEntry::drop`]'s drain waited for a callback on a pool
        /// suspected of not dispatching, on the healer's only thread. That
        /// stops self-heal for every pool in the process.
        ///
        /// A count cannot be fooled that way: it is raised when a repair is
        /// handed over and lowered when one begins, so it is zero exactly when
        /// nothing is queued. The drain then only ever waits for callbacks that
        /// have already started, which complete.
        outstanding: AtomicU32,
        /// Re-submissions made because a repair sat with the pool unstarted.
        ///
        /// Reset the moment a repair callback runs, because that is the
        /// evidence the pool is dispatching again. See
        /// [`note_repair_overdue`](Self::note_repair_overdue).
        stuck_reattempts: AtomicU32,
        /// How many times this entry's own repair item has been dispatched.
        ///
        /// A count rather than a stamp because the stamps answer "in which
        /// order" and this answers "at all", which is what a test asserting the
        /// pre-created object was used needs.
        runs: AtomicU64,
    }

    impl PoolEntry {
        /// The pool this entry is about.
        pub(crate) fn key(&self) -> PoolKey {
            self.key
        }

        /// The work object a repair submits.
        pub(crate) fn repair_work(&self) -> PTP_WORK {
            self.repair.work.load(Ordering::SeqCst)
        }

        /// How many times this pool's own repair item has been dispatched.
        ///
        /// Only this entry's pre-created object can raise it, so a tick that
        /// made a fresh work object and submitted that instead would leave it
        /// at zero -- which is the claim the counter exists to guard.
        // Read only by tests: the lib-only build has no caller, so without this
        // the dead-code warning fires in the configuration CI builds.
        #[cfg(test)]
        pub(crate) fn repairs_run(&self) -> u64 {
            self.runs.load(Ordering::SeqCst)
        }

        /// Note that a cancellation may have severed this pool.
        ///
        /// `fetch_max`, so the stamp keeps the newest cancellation whatever
        /// order concurrent cancellers publish in -- see the field, where the
        /// interleaving that a plain store loses is written out. Unlike the
        /// compare-exchange the flag used, this never silently does nothing
        /// when it matters: it declines only a value already superseded.
        pub(crate) fn stamp_cancelled(&self, at: u64) {
            self.last_cancelled.fetch_max(at, Ordering::SeqCst);
        }

        /// Note when a repair dispatch was observed.
        ///
        /// Only the stamp. Counting lives in
        /// [`count_repair_run`](Self::count_repair_run), because the two
        /// answer different questions: this one orders a dispatch against a
        /// cancellation, and the count says whether this entry's own
        /// pre-created object was the thing submitted. A test that drives the
        /// ordering directly must be able to do so without claiming a repair
        /// ran.
        pub(crate) fn stamp_started(&self, at: u64) {
            self.last_started.fetch_max(at, Ordering::SeqCst);
        }

        /// Note that a repair has begun running, so one fewer is queued.
        ///
        /// Separate from the stamp because the stamp is `fetch_max` and may
        /// decline a value, where this must lower the count exactly once per
        /// dispatch. Saturating rather than a bare `fetch_sub`: a wrap would
        /// turn "nothing queued" into "four billion queued" and pin the entry
        /// in the registry forever, which is a worse failure than the
        /// double-decrement it would be covering for.
        pub(crate) fn note_repair_started(&self) {
            // `fetch_update` is deprecated on current stable, renamed to
            // `try_update`. The new name postdates this workspace's MSRV
            // (1.98), so using it would trade a lint for a build failure on the
            // toolchain the MSRV job pins; the old name compiles on both. See
            // the identical note in `windows-overlapped-io-sys`' `identity.rs`
            // -- both revert when the MSRV moves past the rename.
            #[allow(deprecated)]
            let _ = self
                .outstanding
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| {
                    Some(n.saturating_sub(1))
                });
        }

        /// Count a dispatch of this entry's own repair item.
        ///
        /// Separate from the stamp and the count because a test needs to stop
        /// *between* them: those two are what make `repair_in_flight` false, so
        /// the instant after them is the instant the entry becomes retirable
        /// while its callback is still running. That is the window
        /// `PoolEntry::drop`'s drain exists to cover.
        fn count_repair_run(&self) {
            self.runs.fetch_add(1, Ordering::SeqCst);
            // The pool dispatched, which is the only direct evidence it is not
            // wedged, so whatever overdue episodes preceded this are spent.
            self.stuck_reattempts.store(0, Ordering::SeqCst);
        }

        /// Note that a repair has been handed to the pool.
        ///
        /// Raises `outstanding` as well as stamping the time, because the two
        /// must not be able to disagree about whether a submission happened.
        /// Called immediately before `SubmitThreadpoolWork`, which cannot fail,
        /// so the count matches the dispatches one for one.
        ///
        /// The stamp is a `fetch_max` because callers are not serialised: see
        /// the field's own documentation. The count is a plain `fetch_add`
        /// regardless, since every submission must be counted even when its
        /// timestamp loses the race.
        pub(crate) fn stamp_submitted(&self, at: u64) {
            self.last_submitted.fetch_max(at, Ordering::SeqCst);
            self.outstanding.fetch_add(1, Ordering::SeqCst);
        }

        /// Move the submission stamp *backwards*, for tests only.
        ///
        /// Reaching the overdue state honestly would mean waiting out the real
        /// threshold on every test that needs it. A plain store is what makes
        /// that possible, and is exactly what production must not do -- so the
        /// two are separate functions rather than one with a comment, and only
        /// this one is compiled into a test build.
        ///
        /// Raises `outstanding` like its production twin, because a backdated
        /// stamp with nothing outstanding is not a state the tick can reach and
        /// would send the test down the submit path instead of the overdue one.
        #[cfg(test)]
        pub(crate) fn backdate_submitted(&self, at: u64) {
            self.last_submitted.store(at, Ordering::SeqCst);
            self.outstanding.fetch_add(1, Ordering::SeqCst);
        }

        /// Whether a cancellation stands unanswered by a repair dispatch.
        ///
        /// **Equality counts as unhealed, deliberately.** `QueryInterruptTime`
        /// has system-tick resolution -- about 15.6 ms -- so a cancellation and
        /// a dispatch inside the same tick carry the same stamp, and nothing
        /// here can say which came first. Reading that as healthy would risk
        /// declaring a pool repaired by a dispatch that actually preceded the
        /// cancellation. Reading it as unhealed costs one redundant repair,
        /// which this crate has already established is the safe direction.
        ///
        /// A pool that was never cancelled has a zero stamp and is healthy,
        /// which is why the zero is tested rather than left to the comparison.
        pub(crate) fn unhealed(&self) -> bool {
            let cancelled = self.last_cancelled.load(Ordering::SeqCst);
            cancelled != 0 && cancelled >= self.last_started.load(Ordering::SeqCst)
        }

        /// Whether a submitted repair has been with the pool longer than
        /// `after`, in the units [`now`] counts.
        ///
        /// Distinct from [`repair_in_flight`](Self::repair_in_flight), which is
        /// true the instant a repair is handed over. A repair that is merely
        /// queued behind other work is the ordinary case and says nothing; one
        /// that is still unstarted much later is the case worth reporting.
        pub(crate) fn repair_overdue(&self, now: u64, after: u64) -> bool {
            self.repair_in_flight()
                && now.saturating_sub(self.last_submitted.load(Ordering::SeqCst)) >= after
        }

        /// Count one overdue episode, reporting how many have now been seen
        /// since the last repair actually ran.
        pub(crate) fn note_repair_overdue(&self) -> u32 {
            self.stuck_reattempts.fetch_add(1, Ordering::SeqCst) + 1
        }

        /// Whether a submitted repair has not yet begun running.
        ///
        /// This is also the retirement guard, and that is why it counts rather
        /// than comparing stamps: dropping the last `Arc` runs
        /// `PoolEntry::drop`, whose drain cannot return until the pool
        /// dispatches, and `tick` runs on a healer with one thread. A predicate
        /// that reads false with a repair still queued therefore parks
        /// self-heal for the whole process -- see the `outstanding` field.
        pub(crate) fn repair_in_flight(&self) -> bool {
            self.outstanding.load(Ordering::SeqCst) > 0
        }

        /// How many repairs are outstanding, for tests only.
        ///
        /// [`repair_in_flight`](Self::repair_in_flight) is the production
        /// question and is deliberately a boolean. A test asserting that every
        /// concurrent submission was counted needs the number, and reading it
        /// through the boolean would pass with one.
        #[cfg(test)]
        pub(crate) fn outstanding_for_test(&self) -> u32 {
            self.outstanding.load(Ordering::SeqCst)
        }

        /// When a cancellation last left this pool unrepaired; zero if never.
        // Read only by tests -- see the note on `repairs_run`.
        #[cfg(test)]
        pub(crate) fn last_cancelled(&self) -> u64 {
            self.last_cancelled.load(Ordering::SeqCst)
        }

        /// When a repair was last handed to the pool; zero if never.
        // Read only by tests -- see the note on `repairs_run`.
        #[cfg(test)]
        pub(crate) fn last_submitted(&self) -> u64 {
            self.last_submitted.load(Ordering::SeqCst)
        }

        /// Overdue episodes counted since the last repair actually ran.
        // Read only by tests -- see the note on `repairs_run`.
        #[cfg(test)]
        pub(crate) fn repair_overdue_count(&self) -> u32 {
            self.stuck_reattempts.load(Ordering::SeqCst)
        }

        /// When this entry's repair callback last ran; zero if never.
        // Read only by tests -- see the note on `repairs_run`.
        #[cfg(test)]
        pub(crate) fn last_started(&self) -> u64 {
            self.last_started.load(Ordering::SeqCst)
        }

        /// Record a dispatch strictly after the last cancellation.
        ///
        /// The discharge operation tests need now that there is no clear. It
        /// takes the cancellation stamp and steps past it rather than reading
        /// the clock, so it heals the entry whatever the tick resolution does
        /// -- which `stamp_started(now())` would not, since a cancellation in
        /// the same tick compares equal and equality reads as unhealed.
        #[cfg(test)]
        pub(crate) fn force_healed(&self) {
            self.stamp_started(self.last_cancelled.load(Ordering::SeqCst) + 1);
        }
    }

    /// A `TP_WORK` owned by an entry rather than by a [`crate::work::ThreadpoolWork`].
    ///
    /// Raw rather than the safe type because the safe type registers itself,
    /// which would recurse: creating an entry would create an object, which
    /// would create an entry.
    struct RepairWork {
        /// The handle, or zero before [`arm_repair`] has set it.
        ///
        /// Settable after construction because the work object's callback
        /// context is the *entry's* address, so the entry has to exist before
        /// the object can be created. An entry whose arming failed keeps a zero
        /// here and is never published.
        work: AtomicIsize,
    }

    // SAFETY: a PTP_WORK is a pool object the thread pool itself uses across
    // threads; this wrapper only stores it, submits it, and closes it once.
    unsafe impl Send for RepairWork {}
    unsafe impl Sync for RepairWork {}

    impl Drop for PoolEntry {
        /// Drain the repair object, then close it, before any of this entry is
        /// dropped.
        ///
        /// **The ordering is enforced by the language rather than by a
        /// convention.** `Drop::drop` runs to completion before any field is
        /// dropped and long before the `Arc` releases the allocation, so the
        /// drain below cannot be outrun by the teardown of anything the
        /// callback writes to. That is why this lives here and not on
        /// `RepairWork`.
        ///
        /// It was on `RepairWork`, with `repair` declared first so that field's
        /// drop ran before the others. That worked, but it rested on a
        /// declaration order nothing checks, and it had already been reasoned
        /// about wrongly twice: once when the callback's context was a
        /// `Box<AtomicU64>` -- where the order genuinely mattered, because
        /// dropping that field freed heap -- and once after the box was removed,
        /// when the same justification was restated for fields that free
        /// nothing. Here there is no order to get wrong, and a later field that
        /// *does* own heap cannot silently reintroduce the hazard.
        ///
        /// Draining before the close is still this body's own correctness.
        /// `CloseThreadpoolWork` does not wait: it frees the work object
        /// asynchronously once outstanding callbacks finish, so a repair
        /// already running would go on to stamp an entry whose allocation the
        /// `Arc` had released. The window is not theoretical -- `tick` submits
        /// and then `retire_idle` can drop the last `Arc` microseconds later,
        /// and the pool it submitted to is by construction the one suspected of
        /// dispatching late. `retiring_an_entry_waits_for_a_repair_callback_already_running`
        /// measures it and `sabotage.json` re-checks it.
        fn drop(&mut self) {
            // SAFETY: `work` was created by `CreateThreadpoolWork` in
            // `arm_repair` and is closed exactly once, here. Waiting without
            // cancelling cannot orphan anything: the callback owns no storage.
            let work = self.repair.work.load(Ordering::SeqCst);
            if work == 0 {
                // Never armed: creating the work object failed, so there is
                // nothing to drain and nothing to close.
                return;
            }
            crate::trace_call!("WaitForThreadpoolWorkCallbacks", work, 0, {
                unsafe { WaitForThreadpoolWorkCallbacks(work, FALSE) };
            });
            crate::trace_record!("heal", "repair-closed", work);
            // SAFETY: as above, and no callback can still be running after the
            // drain, so the entry the context names is unreachable from one.
            unsafe { CloseThreadpoolWork(work) };
        }
    }

    /// The repair item's callback, which deliberately does nothing.
    ///
    /// The repair is the *submission*, not the work: a submit is what releases a
    /// stalled pool, and the callback running is only the evidence that it did.
    unsafe extern "system" fn repair_trampoline(
        _instance: PTP_CALLBACK_INSTANCE,
        context: *mut core::ffi::c_void,
        _work: PTP_WORK,
    ) {
        // Counted unconditionally rather than through the trace, because the
        // trace is narrowed by an environment variable that neither the harness
        // nor CI sets -- a check written against it would pass while observing
        // nothing, which is the trap this crate's sabotage manifest documents.
        //
        // SAFETY: the context is the address of the `PoolEntry` that owns this
        // work object, which outlives every dispatch of it: `PoolEntry::drop`
        // drains this object before any field of the entry is dropped, and
        // long before the `Arc` releases the allocation. The argument is the
        // language's ordering of `Drop::drop` against field drops, not a
        // declaration order -- which is what it used to rest on, and what
        // `M-T13.7` corrected here after the drain moved off `RepairWork`.
        let entry = unsafe { &*context.cast::<PoolEntry>() };
        // The stamp and the count first: together they are what make
        // `repair_in_flight` false, so from here the entry can be retired while
        // this callback is still running -- which is the window the drain in
        // `PoolEntry::drop` exists to cover. The count is lowered here rather
        // than when the callback returns for exactly that reason: a repair that
        // has *started* no longer needs the entry held for it, and holding one
        // until its callback returned would mean a retirement could never
        // overlap a running repair, which is the case the drain is written for.
        entry.stamp_started(now());
        entry.note_repair_started();
        #[cfg(test)]
        if entry.key != 0 && HOLD_REPAIR_CALLBACK_FOR.load(Ordering::SeqCst) == entry.key {
            REPAIR_CALLBACK_INSIDE.store(entry.key, Ordering::SeqCst);
            let hold = HOLD_REPAIR_CALLBACK_MS.load(Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(hold));
        }
        // The write that a missing drain would make through a freed entry.
        entry.count_repair_run();
        crate::trace_record!("heal", "repair-ran", _work);
    }

    /// One object's claim on a pool's entry, released when the object goes.
    ///
    /// **The key is kept even when the entry is absent.** An earlier version
    /// held only the `Option`, so an object whose registration failed had no
    /// way to name its own pool afterwards: it could not find an entry another
    /// object created later, and the `cancel-untracked` record it emitted could
    /// not say which pool it was about. Both followed from discarding the key
    /// on the one path that most needed it.
    pub(crate) struct Registration {
        /// The pool this claim is against, known whether or not it registered.
        key: PoolKey,
        /// The entry, when one could be created or found.
        entry: Option<Arc<PoolEntry>>,
    }

    impl Registration {
        /// The entry, when the pool could be registered.
        // No caller yet in any configuration. Kept because it is the only way
        // to reach the entry a registration holds, which the repair paths need
        // as they grow; silenced rather than deleted so the accessor stays
        // available instead of being re-derived later.
        #[allow(dead_code)]
        pub(crate) fn entry(&self) -> Option<&Arc<PoolEntry>> {
            self.entry.as_ref()
        }

        /// Take a second, independent claim on this object's pool.
        ///
        /// Registers the pool when this registration is empty, so the claim
        /// owns a repair work object either way -- and that object is bound to
        /// the pool, which is what makes holding the claim keep the pool alive.
        ///
        /// `CleanupGroup` uses this to close a window its own release opens:
        /// see `ThreadpoolWait::recover_repair`.
        #[must_use]
        pub(crate) fn reclaim(&self) -> Registration {
            register(self.key)
        }

        /// Note that a cancellation on this object's pool owes it a repair.
        ///
        /// Called *after* the cancelling call returns, not before. The question
        /// a repair asks is whether the pool has dispatched since the removal
        /// that may have severed its notification, and a dispatch that happened
        /// while the removal was in progress is no evidence about afterwards.
        ///
        /// Reports whether the pool ended up tracked. `false` means the
        /// cancellation has happened with nothing that will ever repair it,
        /// which is the one case `try_cancel_pending`'s safety claim does not
        /// cover; the caller decides what to do about it, at a point where a
        /// panic cannot skip work the teardown still owes.
        #[must_use]
        pub(crate) fn owe_repair(&self) -> bool {
            let Some(entry) = &self.entry else {
                return self.owe_repair_untracked();
            };
            mark_and_arrange_repair(entry)
        }

        /// Mark a cancellation on a claim taken *before* a release, where
        /// retrying the registration would be unsound.
        ///
        /// Same as [`owe_repair`](Self::owe_repair) except that it does not
        /// retry. The difference is a memory-safety one and is the whole reason
        /// this exists.
        ///
        /// `CleanupGroup` recovers a claim for each member before releasing
        /// them, because a member whose pool was never registered holds nothing
        /// that keeps that pool alive and the release frees the last bound
        /// object. A claim that **holds an entry** keeps the pool alive across
        /// the release -- the entry's repair work object is a bound object, and
        /// `CloseThreadpool` defers the free until every one is gone -- so
        /// marking it afterwards is safe.
        ///
        /// A claim that holds **no** entry pins nothing, which is exactly the
        /// case `owe_repair`'s retry must not be reached in: by the time the
        /// marking pass runs the pool may already have been freed, and
        /// `register` would call `CreateThreadpoolWork` with an environment
        /// naming it. So this reports the cancellation as untracked rather than
        /// trying again, and the caller's fail-fast says so.
        ///
        /// Reachable only when an allocation failed, which is why it is stated
        /// here rather than assumed away.
        #[must_use]
        pub(crate) fn owe_repair_claimed(&self) -> bool {
            let Some(entry) = &self.entry else {
                crate::trace_record!("heal", "cancel-untracked", self.key);
                return false;
            };
            mark_and_arrange_repair(entry)
        }

        /// The cancellation path for an object that never got an entry.
        ///
        /// Registration is attempted **again, here**, which is the one place in
        /// this crate that allocates at the moment of need rather than ahead of
        /// it. The usual argument against that is recorded on `tick`: the repair
        /// work object is pre-created because the pool it is aimed at may
        /// already be wedged. It does not apply to the decision this path faces,
        /// because the alternative is not "allocate later", it is "never repair
        /// this pool at all". Reaching here already means an allocation failed
        /// when the object was created *and* that nothing else on the pool has
        /// registered since, so the choice is between one more small allocation
        /// and leaving a possibly-severed pool with nothing watching it.
        ///
        /// Creating a work object is not dispatching through the pool, so a
        /// wedged pool is not itself a reason for this to fail.
        ///
        /// The temporary claim is dropped at the end: marking first is what
        /// keeps the entry alive through it, because `release` retains an entry
        /// whose repair is owed.
        #[cold]
        fn owe_repair_untracked(&self) -> bool {
            let recovered = register(self.key);
            let Some(entry) = &recovered.entry else {
                // Three allocations have now failed for this pool: the one at
                // the object's creation, any another object might have made
                // since, and the one immediately above. Recorded with the key,
                // so a capture names the pool rather than reporting `0`.
                crate::trace_record!("heal", "cancel-untracked", self.key);
                return false;
            };
            mark_and_arrange_repair(entry)
        }
    }

    /// Mark a cancellation and make sure something will actually repair it.
    ///
    /// **The common tail of all three cancellation paths**, and one site rather
    /// than three because the interesting half is a fallback that is easy to
    /// add to one of them and forget in the others -- which is how this was
    /// first written, and what a review caught.
    ///
    /// The stamp comes first, never after the start: the healer's first tick
    /// must not be able to run before the entry it exists to repair says it is
    /// unhealed. `ensure_running` is called without the registry lock held,
    /// because creating the healer registers its own pool and so takes it.
    ///
    /// **When the healer cannot be started, the repair is submitted here
    /// instead.** Leaving it would mark a pool unhealed and schedule nothing
    /// while still reporting the cancellation as tracked, so
    /// `try_cancel_pending`'s claim that this crate repairs the pool afterwards
    /// would silently not hold and the untracked fail-fast would not fire
    /// either. A later cancellation anywhere in the process retries the start,
    /// which is `M-T10.22`; what that does not cover is a process where no
    /// later cancellation arrives.
    ///
    /// Submitting now is what a tick would have done, it is the action measured
    /// to release the stall, and the work object already exists -- so the only
    /// thing given up is the coalescing the timer provides, which this path
    /// pays for once rather than being a correctness question.
    ///
    /// Always reports the cancellation as tracked: either a healer will visit
    /// this entry or its repair has already been handed over.
    fn mark_and_arrange_repair(entry: &PoolEntry) -> bool {
        entry.stamp_cancelled(now());
        if ensure_running() {
            return true;
        }
        crate::trace_record!("heal", "repair-submitted-inline", entry.key());
        submit_repair(entry);
        true
    }

    /// The interrupt-time counter, which every stamp here is measured on, read
    /// through `win-time-sys`' `InterruptClock` -- the workspace's one reader of
    /// its one time base -- as a raw count of 100 ns ticks for the atomics.
    ///
    /// `QueryInterruptTime` rather than `QueryPerformanceCounter` because the
    /// only question asked of these values is which of two came first: the
    /// counter is read from memory the kernel publishes to user mode, where
    /// `QueryPerformanceCounter` may do more. This used to run on the path of
    /// every callback the crate delivered, which was most of the argument for
    /// the cheaper read; `M-T9.2` removed that stamp, so it now runs only on a
    /// cancellation and on the repair lifecycle. The choice stands because the
    /// question did not change.
    /// Reading `KUSER_SHARED_DATA` directly would be the same read and is how
    /// this is often done, but it binds to a layout nothing promises; the
    /// documented call is the specified primitive for the same value.
    ///
    /// # Resolution, and which way its error falls
    ///
    /// The counter advances on the system clock tick -- tens of milliseconds by
    /// default -- so a cancellation and a repair's start within one tick carry
    /// equal stamps. [`PoolEntry::unhealed`] treats that as unhealed, so a
    /// repair is submitted rather than skipped. That is the
    /// direction the error has to fall: a redundant repair costs one work
    /// submission, where a suppressed one leaves a pool stalled.
    ///
    /// **The counter never goes backwards; publishing it can.** An earlier
    /// revision of this comment concluded from the first fact that "a dispatch
    /// stamp can never exceed a cancellation that followed it", and that does
    /// not follow: reading the clock and storing the value are two steps, so a
    /// thread preempted between them can publish a stale reading after a newer
    /// one has landed. The stamps are therefore written with `fetch_max` rather
    /// than a store, which is what makes the conclusion true -- see
    /// [`PoolEntry::stamp_cancelled`]. Resolution was never the risk; ordering
    /// was.
    pub(crate) fn now() -> u64 {
        InterruptClock.now().ticks()
    }

    impl Drop for Registration {
        fn drop(&mut self) {
            if let Some(entry) = self.entry.take() {
                release(&entry);
            }
        }
    }

    fn registry() -> &'static Mutex<Vec<Arc<PoolEntry>>> {
        static REGISTRY: OnceLock<Mutex<Vec<Arc<PoolEntry>>>> = OnceLock::new();
        REGISTRY.get_or_init(|| Mutex::new(Vec::new()))
    }

    /// Lock the registry, recovering from a panicking holder.
    ///
    /// A poisoned registry would otherwise disable self-heal for the rest of the
    /// process, which is a worse outcome than proceeding: the entries are plain
    /// counters and atomics with no invariant a panic can leave half-applied.
    fn locked() -> std::sync::MutexGuard<'static, Vec<Arc<PoolEntry>>> {
        registry()
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    /// Claim a pool's entry on behalf of one object, creating it if needed.
    ///
    /// **Best-effort.** If the repair work object cannot be created the pool goes
    /// unregistered and the returned registration is empty; object creation still
    /// succeeds, because failing it would turn an unrelated allocation failure
    /// into a failure of the caller's actual request.
    ///
    /// **An empty registration is not final.** It keeps its key, and
    /// [`Registration::owe_repair`] calls this again when a cancellation
    /// arrives, so a pool that could not be registered when one object was
    /// created is registered then -- either finding an entry something else
    /// made in the meantime, or making one. Only when that call fails too is a
    /// cancellation performed with nothing to repair it, which is the one case
    /// where `try_cancel_pending`'s safety claim does not hold; it records
    /// `cancel-untracked` and, under `fail-fast`, panics. Decided in
    /// `M-T10.18`.
    ///
    /// Note that this returns **without incrementing `objects`** when it fails,
    /// so an object holding an empty registration is not counted by the
    /// refcount and an entry can retire while it is still live. That is why the
    /// retry re-registers rather than only looking the key up.
    pub(crate) fn register(key: PoolKey) -> Registration {
        let mut entries = locked();
        if let Some(entry) = entries.iter().find(|entry| entry.key == key) {
            entry.objects.fetch_add(1, Ordering::Relaxed);
            crate::trace_record!(
                "heal",
                "registered",
                key,
                entry.objects.load(Ordering::Relaxed)
            );
            return Registration {
                key,
                entry: Some(Arc::clone(entry)),
            };
        }
        // Built before the work object, because the work object's callback
        // context is this entry's own address. The entry is not published until
        // arming succeeds, so a failed arming leaves nothing behind: the local
        // `Arc` is dropped here, and `PoolEntry::drop` sees a zero handle and
        // does nothing.
        let entry = Arc::new(PoolEntry {
            repair: RepairWork {
                work: AtomicIsize::new(0),
            },
            key,
            objects: AtomicUsize::new(1),
            last_cancelled: AtomicU64::new(0),
            last_started: AtomicU64::new(0),
            last_submitted: AtomicU64::new(0),
            outstanding: AtomicU32::new(0),
            stuck_reattempts: AtomicU32::new(0),
            runs: AtomicU64::new(0),
        });
        if !arm_repair(&entry) {
            crate::trace_record!("heal", "register-failed", key);
            return Registration { key, entry: None };
        }
        crate::trace_record!("heal", "entry-created", key, entry.repair_work());
        entries.push(Arc::clone(&entry));
        Registration {
            key,
            entry: Some(entry),
        }
    }

    /// Whether an entry can be dropped from the registry.
    ///
    /// **One definition, because there are two retirement sites.** `release`
    /// retires when an object goes and `retire_idle` retires when a repair is
    /// discharged, and for a while they disagreed: the outstanding-repair
    /// condition was added to the second and not the first, so the hazard it
    /// was added for -- `PoolEntry::drop` draining a repair the pool has not
    /// dispatched, on the healer's only thread -- was still reachable through
    /// the other. Two copies of a rule that must agree is the shape this
    /// repository treats as a defect; this is the shape that cannot have it.
    pub(crate) fn is_retirable(entry: &PoolEntry) -> bool {
        entry.objects.load(Ordering::Relaxed) == 0 && !entry.unhealed() && !entry.repair_in_flight()
    }

    /// Give up one object's claim, retiring the entry when nothing needs it.
    ///
    /// **An entry outlives its objects while a repair is owed**, and that single
    /// rule covers the default pool and a private one alike -- see the decision
    /// recorded with `M-T6.1`. Retiring an entry that still owes a repair would
    /// drop the repair at exactly the moment it is needed.
    fn release(entry: &Arc<PoolEntry>) {
        // Moved out rather than dropped in place; see `retire_idle`.
        let retired: Vec<Arc<PoolEntry>> = {
            let mut entries = locked();
            let remaining = entry.objects.fetch_sub(1, Ordering::Relaxed) - 1;
            if !is_retirable(entry) {
                crate::trace_record!("heal", "released", entry.key, remaining);
                return;
            }
            crate::trace_record!("heal", "entry-retired", entry.key, remaining);
            let (out, keep) = entries
                .iter()
                .cloned()
                .partition(|held| held.key == entry.key);
            *entries = keep;
            out
        };
        drop(retired);
    }

    /// Retire every entry that has no objects, owes no repair, and has no
    /// repair still sitting in its pool.
    ///
    /// For the self-heal timer (`M-T6.4`), which is what discharges a repair and
    /// so is what makes a retained entry retirable again.
    ///
    /// **An entry with a submission still outstanding is kept**, however idle it
    /// otherwise looks. Retiring it drops the last `Arc`, which runs
    /// `PoolEntry::drop`, which drains the work object -- and that drain cannot
    /// return until the pool dispatches the repair. The pool in question is by
    /// construction the one suspected of not dispatching, and `tick` runs on a
    /// healer built with `set_max_threads(1)`, so a drain that blocks there
    /// parks the only thread the facility has and no pool in the process is
    /// ever repaired again. Keeping one entry alive for a pool that never
    /// dispatches is the cheaper failure by a wide margin.
    ///
    /// Moving the drop out of the registry lock, below, is a separate and
    /// still-necessary precaution: it stops such a wait blocking every other
    /// pool's registration and release. It does not help the healer's own
    /// thread, which is what this condition is for.
    pub(crate) fn retire_idle() {
        // The retired entries are moved out under the lock and dropped after it
        // is released.
        //
        // Dropping the last `Arc` in place would run `PoolEntry::drop`, which
        // now drains the work object before closing it -- and that drain waits
        // on a pool this feature only touches because it is suspected of not
        // dispatching. Holding the process-wide registry lock across such a
        // wait would let one wedged pool stall every other pool's registration
        // and release.
        let retired: Vec<Arc<PoolEntry>> = {
            let mut entries = locked();
            let (out, keep) = entries.iter().cloned().partition(|entry| {
                let idle = is_retirable(entry);
                if idle {
                    crate::trace_record!("heal", "entry-retired", entry.key, 0);
                }
                idle
            });
            *entries = keep;
            out
        };
        drop(retired);
    }

    /// Every entry currently registered.
    ///
    /// For the self-heal timer, and for tests. Clones the `Arc`s so the lock is
    /// not held while a caller works through them -- a repair submits to a pool
    /// that may be wedged, which must not be done holding a process-wide lock.
    pub(crate) fn entries() -> Vec<Arc<PoolEntry>> {
        locked().clone()
    }

    /// How often the healer looks for a pool owing a repair.
    ///
    /// With [`HEAL_WINDOW`] this bounds the repair latency at roughly their sum.
    /// The thing being bounded is a pool that would otherwise stay undeliverable
    /// until the application happened to submit work -- which, for a program
    /// built on waits, timers and I/O, is never.
    const HEAL_PERIOD: Duration = Duration::from_millis(250);

    /// Coalescing tolerance the system may add to each tick.
    ///
    /// A repair is not urgent to the millisecond, and this lets the system group
    /// the wakeup with others rather than taking one of its own.
    const HEAL_WINDOW: Duration = Duration::from_millis(250);

    /// The private pool and timer that perform repairs.
    ///
    /// Held in a `static` and never dropped, which is deliberate: Rust does not
    /// run destructors for statics, so there is no teardown path to get wrong,
    /// and the alternative -- tearing down a thread pool at process exit while
    /// callbacks may still be dispatching -- is the hazard this whole mechanism
    /// exists to avoid.
    struct Healer {
        _pool: crate::pool::ThreadpoolPool,
        _timer: crate::timer::ThreadpoolPeriodicTimer,
    }

    // SAFETY: both members are Send + Sync; this only keeps them alive.
    unsafe impl Send for Healer {}
    unsafe impl Sync for Healer {}

    /// Start the healer if it is not already running.
    ///
    /// Lazy, so a consumer who never cancels never creates a pool or a thread.
    ///
    /// **Once started it runs until the process exits**, rather than stopping
    /// when nothing is owed. Stopping would be cheaper and is not safe without a
    /// lock the cancel path should not pay for: a tick that found nothing owed
    /// could stop the timer *after* a concurrent cancellation had marked its
    /// pool and asked for the healer, leaving a repair owed with nothing to
    /// deliver it. A coalesced tick is the cheaper of the two mistakes.
    /// **Only success is remembered.** This used to be an
    /// `OnceLock<Option<Healer>>` filled by `get_or_init`, which cached the
    /// *failure* too: one transient `ThreadpoolPool::new` under memory
    /// pressure, at the first cancellation in the process, and the healer was
    /// never attempted again. Every later cancellation would still mark its
    /// pool, nothing would ever tick, and no failure was reported -- the whole
    /// feature off for the life of the process on the strength of one bad
    /// moment. A failed attempt now records and leaves the slot empty, so the
    /// next cancellation tries again.
    #[must_use]
    fn ensure_running() -> bool {
        /// Fast path, so the common already-running case pays a load rather
        /// than a lock on the cancellation path.
        static RUNNING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        static HEALER: Mutex<Option<Healer>> = Mutex::new(None);

        // Lets a test reach the no-healer path without having to make pool or
        // timer creation fail for real. Read before the fast path, because by
        // the time a test runs some earlier test has almost certainly started
        // the healer for the process.
        #[cfg(test)]
        if FORCE_HEALER_START_FAILURE.load(Ordering::SeqCst) {
            return false;
        }

        if RUNNING.load(Ordering::Acquire) {
            return true;
        }
        // Held across construction, which registers the healer's own pool and
        // so takes the registry lock. That order -- this lock, then the
        // registry -- is the only one taken anywhere: `tick` and `register`
        // take the registry lock and never this one, and `owe_repair` calls
        // here without holding it.
        let mut slot = HEALER.lock().unwrap_or_else(|poison| poison.into_inner());
        if slot.is_some() {
            return true;
        }
        let Some(healer) = build() else {
            crate::trace_record!("heal", "healer-start-failed", 0);
            return false;
        };
        crate::trace_record!("heal", "healer-started", 0);
        *slot = Some(healer);
        RUNNING.store(true, Ordering::Release);
        true
    }

    /// Create the healer's pool and periodic timer, or report that it could not.
    fn build() -> Option<Healer> {
        let pool = crate::pool::ThreadpoolPool::new().ok()?;
        // One thread is enough: a tick submits and returns.
        pool.set_max_threads(1).ok()?;
        // Scoped so the environment's borrow of `pool` ends before `pool`
        // is moved into the value the static keeps. The borrow is real --
        // `set_pool` ties the environment to the pool it names -- and only
        // the construction needs it.
        let timer = {
            let mut env = crate::callback_env::CallbackEnviron::new();
            env.set_pool(&pool);
            crate::timer::ThreadpoolPeriodicTimer::new(HEAL_PERIOD, |_tick| tick(), Some(&mut env))
                .ok()?
        };
        timer.start_with_window(HEAL_PERIOD, HEAL_WINDOW);
        Some(Healer {
            _pool: pool,
            _timer: timer,
        })
    }

    /// Serializes a tick against a test's mark-then-assert.
    ///
    /// `tick` processes **every** registered pool, so it is not isolated by a
    /// test making its own: one test's tick -- or the background healer's,
    /// which starts at the first cancellation anywhere in the process -- can
    /// clear another test's mark between the cancellation and the assertion
    /// about it. `cargo test` runs these as threads of one process, which is
    /// deliberate here, so that is a live hazard rather than a theoretical one:
    /// a review reproduced it on run 19 of a loop at 32 test threads.
    ///
    /// A test holds this across the whole of its critical section and calls
    /// [`tick_inner`] rather than [`tick`], which would deadlock on the gate it
    /// is already holding.
    #[cfg(test)]
    pub(crate) static TICK_GATE: Mutex<()> = Mutex::new(());

    /// One pass over the registry: repair what is owed, skip what is alive.
    pub(crate) fn tick() {
        #[cfg(test)]
        let _gate = TICK_GATE
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        tick_inner();
    }

    /// How long a submitted repair may go unstarted before it is reported.
    ///
    /// In the 100-nanosecond units [`now`] counts, so this is five seconds --
    /// twenty healer periods.
    ///
    /// **Generous on purpose.** Two states look identical from here: a pool
    /// that is wedged, and a pool that is merely busy. The second is enormously
    /// more likely, and the cost of waiting longer is only a later report,
    /// where the cost of being hasty is crying wolf about a healthy pool under
    /// load -- and, under `fail-fast`, ending the process over it.
    const OVERDUE_AFTER: u64 = 5 * 10_000_000;

    /// Overdue episodes tolerated before `fail-fast` gives up.
    ///
    /// One, so there is a single reattempt. The first overdue report re-submits
    /// and says so; a second means the reattempt did not help either, and at
    /// that point this crate has no further idea whether the pool can be
    /// recovered -- which is the condition `fail-fast` exists to stop on.
    /// Reset by a repair actually running, so this counts one *episode* rather
    /// than one process lifetime.
    const REATTEMPTS_BEFORE_FAIL_FAST: u32 = 1;

    /// Hand this entry's pre-created repair to its pool.
    ///
    /// **One definition, because there are two places a repair is submitted.**
    /// The healer's tick is the ordinary one; `Registration::owe_repair` is the
    /// other, on the path where the healer could not be started and a tick will
    /// therefore never arrive. The two must agree about the order of the stamp,
    /// the count and the call, and the order is the subtle part -- so they ask
    /// the same function rather than restating it.
    ///
    /// The work object was made when the pool was registered, so this allocates
    /// nothing and makes one call, which matters because the pool it is aimed
    /// at may already be wedged. It is never null: `register` returns an empty
    /// registration and keeps the entry out of the registry when `arm_repair`
    /// fails, so an entry that can be reached from either caller has one.
    fn submit_repair(entry: &PoolEntry) {
        crate::trace_record!("heal", "repair-submitted", entry.key(), entry.repair_work());
        // Counted and stamped *before* the submit, never after: the repair can
        // be dispatched on a pool thread the instant it is handed over, so a
        // trampoline that lowered the count before this raised it would leave
        // `repair_in_flight` reading false with one outstanding, which is the
        // one answer it must never give. The saturating decrement means such an
        // inversion could not even be recovered by the arithmetic.
        entry.stamp_submitted(now());
        // SAFETY: the work object was created by `arm_repair` for this entry
        // and is alive while the entry is, which the caller's reference keeps.
        unsafe { SubmitThreadpoolWork(entry.repair_work()) };
    }

    /// [`tick`] without taking the test gate.
    ///
    /// **Reads state, and writes only its own stamp.** There is no clear here,
    /// and that absence is the point of `M-T9.2`: the flag this replaces was
    /// set by a compare-exchange that kept the earliest cancellation and
    /// cleared by an unconditional store, so a cancellation arriving between
    /// this function's read and its clear was recorded nowhere. The repair
    /// submitted below happened strictly *before* such a cancellation and
    /// therefore could not answer it, and the pool was left wedged with nothing
    /// scheduled. Both branches had it; the skip was worse, because there
    /// nothing was submitted at all.
    pub(crate) fn tick_inner() {
        for entry in entries() {
            if !entry.unhealed() {
                // Either nothing has cancelled on this pool, or a repair has
                // dispatched since the last one that did. The second case is
                // direct evidence from *our own* work item, which the dispatch
                // stamp it replaces could not give: that depended on a user
                // object's trampoline firing, so a quiet pool looked exactly
                // like a wedged one.
                continue;
            }
            if entry.repair_in_flight() {
                if !entry.repair_overdue(now(), OVERDUE_AFTER) {
                    // Ordinary: a repair handed over moments ago has not run
                    // yet. Submitting another cannot tell us anything the first
                    // will not.
                    crate::trace_record!("heal", "repair-in-flight", entry.key());
                    continue;
                }
                // The repair has been with the pool long enough that "merely
                // queued" has stopped being the likely reading. Report it, and
                // fall through to submit another.
                //
                // **What a second submit is worth is not established.** The
                // recovery this feature rests on is measured for *a* submit;
                // whether a factory re-evaluates its create decision on a later
                // arrival is unanswered, and the captures under `measurements/`
                // record that an ordinary completion-port arrival has never
                // been seen to recover this stall.
                // So the reattempt is a cheap thing tried in a state that
                // should not arise, not a mechanism with evidence behind it.
                let overdue = entry.note_repair_overdue();
                crate::trace_record!("heal", "repair-overdue", entry.key(), u64::from(overdue));
                if overdue > REATTEMPTS_BEFORE_FAIL_FAST {
                    crate::obligation::fail_fast_if_unrepairable(entry.key(), overdue);
                }
            }
            submit_repair(&entry);
        }
        // Entries kept alive only by an owed repair become retirable once it is
        // discharged, and this is the only place that can notice.
        retire_idle();
    }

    /// Force [`arm_repair`] to fail for one pool, so the untracked path can
    /// be tested.
    ///
    /// That path is reached only when `CreateThreadpoolWork` fails, which means
    /// the process is out of memory -- not a condition a test can produce on
    /// demand, and the error edge would otherwise be written but never
    /// executed. This makes it reachable deterministically.
    ///
    /// **Keyed to pools, not a plain on/off flag.** The registry is
    /// process-wide and these tests run as threads in one process, so a boolean
    /// here fails registrations belonging to whichever unrelated test happens
    /// to be creating an object at the time. Measured: it did exactly that on
    /// the first run, failing `a_dispatch_after_the_cancellation_is_what_counts`
    /// rather than anything it had to do with.
    ///
    /// **A list of keys, not one slot.** A single slot kept unrelated tests out
    /// but let the tests that force a failure undo each other: one's key
    /// replaced another's, or one's release cleared the slot while the other
    /// was still relying on it. Measured: an intermittent failure in two of them,
    /// in about one run in thirty. Each [`ForcedRepairFailure`] adds its own
    /// entry and removes only that entry.
    #[cfg(test)]
    static FORCED_REPAIR_FAILURES: Mutex<Vec<usize>> = Mutex::new(Vec::new());

    #[cfg(test)]
    fn forced_repair_failures() -> std::sync::MutexGuard<'static, Vec<usize>> {
        FORCED_REPAIR_FAILURES
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// [`arm_repair`] fails for one pool while this is live and not yet
    /// lifted -- the only way the untracked path can be reached in a test.
    #[cfg(test)]
    pub(crate) struct ForcedRepairFailure {
        key: usize,
        lifted: std::sync::atomic::AtomicBool,
    }

    #[cfg(test)]
    impl ForcedRepairFailure {
        pub(crate) fn for_pool(key: usize) -> Self {
            forced_repair_failures().push(key);
            Self {
                key,
                lifted: std::sync::atomic::AtomicBool::new(false),
            }
        }

        /// End this forcing now rather than at drop; a second call does
        /// nothing. Removes one entry for this key, so another forcing of the
        /// same pool stays in force.
        pub(crate) fn lift(&self) {
            if self.lifted.swap(true, Ordering::SeqCst) {
                return;
            }
            let mut forced = forced_repair_failures();
            if let Some(at) = forced.iter().position(|&key| key == self.key) {
                forced.swap_remove(at);
            }
        }
    }

    #[cfg(test)]
    impl Drop for ForcedRepairFailure {
        fn drop(&mut self) {
            self.lift();
        }
    }

    /// Make [`ensure_running`] report that no healer is available.
    ///
    /// The fallback it gates only runs when the healer cannot be started, and
    /// making pool or timer creation fail for real is not something a test can
    /// ask for. Read before the fast path rather than inside `build`, because
    /// by the time any test runs some earlier one has almost certainly started
    /// the healer for the whole process -- a flag that only reached `build`
    /// would be inert.
    ///
    /// Unkeyed, unlike the two below, and the tick gate is what makes that
    /// safe: a test setting this holds the gate, so no other test's
    /// cancellation is in flight to be perturbed by it.
    #[cfg(test)]
    pub(crate) static FORCE_HEALER_START_FAILURE: std::sync::atomic::AtomicBool =
        std::sync::atomic::AtomicBool::new(false);

    /// The pool whose repair callback is held inside its dispatch, and for how
    /// long.
    ///
    /// The drain in `PoolEntry::drop` covers a window a few instructions wide:
    /// between the callback stamping `last_started` -- which is what makes the
    /// entry retirable -- and the callback returning. Nothing can land a
    /// retirement in that window by timing, so a test widens it.
    ///
    /// **Keyed to one pool, like [`ForcedRepairFailure`] and for the same
    /// reason.** The registry is process-wide and these tests are threads in
    /// one process, so an unkeyed hold catches whichever repair happens to
    /// dispatch -- including one submitted by another test before this one
    /// took the tick gate. Measured: the first version of this used a bare
    /// flag, passed alone, and failed in the full suite because another test's
    /// callback raised it while this test's own repair had not dispatched yet.
    #[cfg(test)]
    pub(crate) static HOLD_REPAIR_CALLBACK_FOR: AtomicUsize = AtomicUsize::new(0);

    /// How long a held callback stays inside the window, in milliseconds.
    #[cfg(test)]
    pub(crate) static HOLD_REPAIR_CALLBACK_MS: AtomicU64 = AtomicU64::new(0);

    /// The pool whose repair callback has reached the window; zero if none.
    ///
    /// A test waits for *its own* key here rather than sleeping, so it drops
    /// the last `Arc` while that callback is demonstrably still running rather
    /// than hoping it is.
    #[cfg(test)]
    pub(crate) static REPAIR_CALLBACK_INSIDE: AtomicUsize = AtomicUsize::new(0);

    /// Give an entry its work object, with the entry itself as the context.
    ///
    /// Reports whether it now has one. The entry must not be published to the
    /// registry until this says `true`.
    ///
    /// The context is the entry's address rather than a side allocation. The
    /// `Box<AtomicU64>` this replaces was justified by its lifetime being
    /// easier to guarantee than the entry's, which was never true: the box was
    /// freed by the same drop that freed everything else, so when `Drop` did
    /// not drain, the box dangled too. It made the use-after-free smaller
    /// rather than absent, and smaller is what kept it hidden.
    fn arm_repair(entry: &Arc<PoolEntry>) -> bool {
        let key = entry.key;
        #[cfg(test)]
        if key != 0 && forced_repair_failures().contains(&key) {
            return false;
        }
        // The address of the entry's allocation, which is stable for as long as
        // any `Arc` to it lives and is what the trampoline casts back.
        let context: *mut core::ffi::c_void =
            Arc::as_ptr(entry).cast::<core::ffi::c_void>().cast_mut();
        let mut env = crate::callback_env::CallbackEnviron::new();
        // SAFETY: a non-zero key names a live pool -- the caller is creating an
        // object against it in this call -- and the environment is used only for
        // the `CreateThreadpoolWork` below, which copies it.
        unsafe { env.set_pool_raw(key as isize) };
        let work = crate::trace_call!("CreateThreadpoolWork", key, 0, {
            // SAFETY: the trampoline matches the required ABI, the context is
            // the entry that owns this work object and outlives every dispatch
            // of it, and the environment is live for this call.
            unsafe { CreateThreadpoolWork(Some(repair_trampoline), context, env.as_mut_ptr()) }
        });
        // `PTP_WORK` is an opaque handle rather than a pointer, so failure is a
        // zero value, not a null one.
        if work == 0 {
            return false;
        }
        entry.repair.work.store(work, Ordering::SeqCst);
        true
    }
}

/// The shape of the API in each feature configuration, asserted by the build.
///
/// The item that queued this (`M-T6.6`) asked for the off build to be checked
/// rather than assumed. A `cargo check` run by hand does that once; these run
/// wherever the crate's doctests run, in whichever configuration it was built
/// with, so neither half can rot unnoticed.
///
/// With `self-heal` **on**, the safe method exists:
///
/// ```
/// # #[cfg(feature = "self-heal")]
/// # fn main() -> std::io::Result<()> {
/// use windows_threadpool_sys::wait::{ThreadpoolWait, WaitableHandle};
/// let wait = ThreadpoolWait::new(WaitableHandle::event(true, false)?, |_| {}, None)?;
/// wait.try_cancel_pending();
/// # Ok(())
/// # }
/// # #[cfg(not(feature = "self-heal"))]
/// # fn main() {}
/// ```
///
/// With `self-heal` **off**, it does not, and calling it is a compile error --
/// which is the designed behaviour rather than an oversight. The block below
/// names the **real** method and is emitted **only in the build where the claim
/// applies**, so it fails to compile for the stated reason rather than for any
/// reason at all.
///
/// An earlier version was a `compile_fail` on a made-up method name, emitted in
/// both configurations. That could not fail for the right reason in either: it
/// asserted only that `rustc` rejects an unknown method, which it would do with
/// the feature on, off, or the crate absent. The `cfg_attr` is what lets the
/// real name be used, because the block then does not exist in the build where
/// the method does.
#[cfg_attr(
    not(feature = "self-heal"),
    doc = "```compile_fail,E0599",
    doc = "use windows_threadpool_sys::wait::{ThreadpoolWait, WaitableHandle};",
    doc = "let wait = ThreadpoolWait::new(",
    doc = "    WaitableHandle::event(true, false).unwrap(),",
    doc = "    |_| {},",
    doc = "    None,",
    doc = ")",
    doc = ".unwrap();",
    doc = "wait.try_cancel_pending();",
    doc = "```"
)]
/// The `unsafe` sibling is present either way, which is what makes it the one a
/// consumer can always reach for:
///
/// ```
/// # fn main() -> std::io::Result<()> {
/// use windows_threadpool_sys::wait::{ThreadpoolWait, WaitableHandle};
/// let wait = ThreadpoolWait::new(WaitableHandle::event(true, false)?, |_| {}, None)?;
/// // SAFETY: nothing here depends on the pool afterwards, and the object is
/// // dropped immediately, so the repair obligation is discharged by not
/// // relying on what it would have repaired.
/// unsafe { wait.try_cancel_pending_no_heal_tracking() };
/// # Ok(())
/// # }
/// ```
#[cfg(doctest)]
pub(crate) struct FeatureShapeDoctests;

#[cfg(test)]
mod tests;
