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
pub(crate) use on::{FORCE_REPAIR_FAILURE_FOR, TICK_GATE, tick_inner};

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
        /// Records nothing: with no entry there is no slot to stamp.
        pub(crate) const fn stamp_dispatch(&self) {}

        /// Records nothing. Without the feature the repair obligation belongs to
        /// the caller of `try_cancel_pending_no_heal_tracking`, not to us.
        ///
        /// Reports `true`: the question the value answers is "did this crate
        /// leave a pool unrepaired behind a safety claim it made", and in this
        /// build the crate makes no such claim, so there is nothing to report.
        /// Answering `false` would fire the untracked fail-fast on every
        /// cancellation in a configuration that never promised a repair.
        #[must_use]
        pub(crate) const fn owe_repair(&self) -> bool {
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
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex, OnceLock};
    use std::time::Duration;
    use windows_sys::Win32::Foundation::FALSE;
    use windows_sys::Win32::System::Threading::{
        CloseThreadpoolWork, CreateThreadpoolWork, PTP_CALLBACK_INSTANCE, PTP_WORK,
        SubmitThreadpoolWork, WaitForThreadpoolWorkCallbacks,
    };
    use windows_sys::Win32::System::WindowsProgramming::QueryInterruptTime;

    /// What this crate knows about one pool.
    ///
    /// The two stamps are compared against each other and never against a
    /// wall-clock, so the only requirement on their source is that it be
    /// monotonic and shared -- see `M-T6.2`, which picks it.
    pub(crate) struct PoolEntry {
        key: PoolKey,
        /// When a dispatch was last observed on this pool. Written by `M-T6.2`.
        ///
        /// A value *after* `repair_owed_at` is direct evidence the pool is still
        /// delivering callbacks, which is what lets the self-heal skip it.
        last_dispatch: AtomicU64,
        /// When a cancellation left a repair owing; zero means none is owed.
        /// Written by `M-T6.3`.
        repair_owed_at: AtomicU64,
        /// How many live objects this crate has on the pool.
        objects: AtomicUsize,
        /// The work item a repair submits. Created with the entry, never later.
        repair: RepairWork,
    }

    impl PoolEntry {
        /// The pool this entry is about.
        pub(crate) fn key(&self) -> PoolKey {
            self.key
        }

        /// The work object a repair submits.
        pub(crate) fn repair_work(&self) -> PTP_WORK {
            self.repair.work
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
            self.repair.runs.load(Ordering::SeqCst)
        }

        /// Note that a repair has been handed to the pool.
        ///
        /// Counted so [`repair_settled`](Self::repair_settled) can tell a
        /// dispatched repair from one the pool still holds.
        fn repair_submitted(&self) {
            self.repair.submitted.fetch_add(1, Ordering::SeqCst);
        }

        /// Whether every repair submitted on this entry has been dispatched.
        ///
        /// False while the pool still holds one, which is the state in which
        /// `WaitForThreadpoolWorkCallbacks` -- the drain in `RepairWork::drop`
        /// -- would block until the pool dispatches it. On a pool that never
        /// does, that wait never returns.
        ///
        /// The counters are only ever compared, never used to pair a particular
        /// submission with a particular run, so the submit-then-dispatch race
        /// can at worst report `false` for an entry that has just settled. That
        /// costs one more pass before the entry retires and cannot report
        /// `true` while a submission is outstanding, which is the direction that
        /// matters.
        pub(crate) fn repair_settled(&self) -> bool {
            self.repair.runs.load(Ordering::SeqCst) >= self.repair.submitted.load(Ordering::SeqCst)
        }

        /// Note that the pool dispatched a callback.
        pub(crate) fn stamp_dispatch(&self, at: u64) {
            self.last_dispatch.store(at, Ordering::Relaxed);
        }

        /// Note that a cancellation left this pool owing a repair.
        ///
        /// Keeps the earliest unrepaired cancellation rather than the latest: the
        /// question a repair asks is whether a dispatch has been seen since the
        /// cancellation, and overwriting with a later stamp could make a dispatch
        /// that genuinely followed the first cancellation look as though it
        /// preceded the second.
        pub(crate) fn owe_repair(&self, at: u64) {
            let _ =
                self.repair_owed_at
                    .compare_exchange(0, at, Ordering::Relaxed, Ordering::Relaxed);
        }

        /// When the oldest unrepaired cancellation happened, or `None`.
        pub(crate) fn repair_owed_at(&self) -> Option<u64> {
            match self.repair_owed_at.load(Ordering::Relaxed) {
                0 => None,
                at => Some(at),
            }
        }

        /// Whether a dispatch has been observed since `at`.
        pub(crate) fn dispatched_since(&self, at: u64) -> bool {
            self.last_dispatch.load(Ordering::Relaxed) > at
        }

        /// When a dispatch was last observed; zero if none has been.
        // Read only by tests -- see the note on `repairs_run`.
        #[cfg(test)]
        pub(crate) fn last_dispatch(&self) -> u64 {
            self.last_dispatch.load(Ordering::Relaxed)
        }

        /// Mark the owed repair discharged.
        pub(crate) fn clear_repair(&self) {
            self.repair_owed_at.store(0, Ordering::Relaxed);
        }
    }

    /// A `TP_WORK` owned by an entry rather than by a [`crate::work::ThreadpoolWork`].
    ///
    /// Raw rather than the safe type because the safe type registers itself,
    /// which would recurse: creating an entry would create an object, which
    /// would create an entry.
    struct RepairWork {
        work: PTP_WORK,
        /// How many times this object has been handed to the pool.
        ///
        /// Paired with `runs` so an entry is retired only once the pool has
        /// given every submission back. See
        /// [`PoolEntry::repair_settled`](PoolEntry::repair_settled).
        submitted: AtomicU64,
        /// How many times *this* object has been dispatched.
        ///
        /// Boxed so its address is stable, and handed to the work object as its
        /// callback context, so only this entry's repair item can increment it.
        /// That is what lets a test tell the pre-created object being submitted
        /// from a fresh one made on the healing path: a work object created
        /// anywhere else carries a different context and cannot touch this
        /// counter. Per entry rather than process-wide because the registry is
        /// shared and `cargo test` runs these as threads in one process, so a
        /// global count could be satisfied by another test's repair and would
        /// prove nothing about this one.
        // Never read through this field: it is held for its *address*, which is
        // the work object's callback context, and for the lifetime that keeps
        // that address valid. The counter is read through the context instead.
        //
        // That lifetime is only long enough because `Drop` drains the work
        // object before closing it. An earlier revision of this comment claimed
        // the close itself waited for a running callback; `CloseThreadpoolWork`
        // does no such thing, and the drain is what makes the claim true.
        #[allow(dead_code)]
        runs: Box<AtomicU64>,
    }

    // SAFETY: a PTP_WORK is a pool object the thread pool itself uses across
    // threads; this wrapper only stores it, submits it, and closes it once.
    unsafe impl Send for RepairWork {}
    unsafe impl Sync for RepairWork {}

    impl Drop for RepairWork {
        fn drop(&mut self) {
            // Drained before the close, and the order is the whole of this
            // impl's correctness.
            //
            // `runs` is a `Box` whose *address* is this work object's callback
            // context, and the field drop below frees it the instant this body
            // returns. `CloseThreadpoolWork` does not wait: it frees the work
            // object asynchronously once outstanding callbacks finish, so a
            // repair that has been submitted and not yet dispatched would run
            // afterwards and `fetch_add` through freed heap.
            //
            // The window is not theoretical. `tick` submits, clears the mark,
            // and then calls `retire_idle`, which can drop the last `Arc` to
            // this entry microseconds later -- and the pool it just submitted
            // to is by construction the one suspected of not dispatching
            // promptly, so the callback is *most* likely to be late on exactly
            // the path that frees its context.
            //
            // `ThreadpoolWork::drop` has always done this correctly; this one
            // did not, and two comments here asserted otherwise.
            //
            // SAFETY: `work` was created by `CreateThreadpoolWork` here and is
            // closed exactly once. Waiting without cancelling cannot orphan
            // anything: the callback owns no storage.
            crate::trace_call!("WaitForThreadpoolWorkCallbacks", self.work, 0, {
                unsafe { WaitForThreadpoolWorkCallbacks(self.work, FALSE) };
            });
            crate::trace_record!("heal", "repair-closed", self.work);
            // SAFETY: as above, and no callback can still be running after the
            // drain, so the context the field drop frees is unreachable.
            unsafe { CloseThreadpoolWork(self.work) };
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
        // SAFETY: the context is the `runs` box of the `RepairWork` owning this
        // object, which outlives every dispatch of it -- the close in `Drop`
        // waits for a running callback before the box is freed.
        unsafe { &*context.cast::<AtomicU64>() }.fetch_add(1, Ordering::SeqCst);
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

        /// Note that this object's pool has just dispatched a callback.
        ///
        /// Called from a trampoline, so it is on the path of every callback the
        /// crate delivers: one counter read and one relaxed store.
        pub(crate) fn stamp_dispatch(&self) {
            if let Some(entry) = &self.entry {
                entry.stamp_dispatch(now());
            }
        }

        /// Note that a cancellation on this object's pool owes it a repair.
        ///
        /// Stamped *after* the cancelling call returns, not before. The question
        /// a repair asks is whether the pool has dispatched since the removal
        /// that may have severed its notification, and a dispatch that happened
        /// while the removal was in progress is no evidence about afterwards.
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
            entry.owe_repair(now());
            // After the mark, never before: the healer's first tick must
            // not be able to run before the entry it exists to repair says
            // it is owed one.
            //
            // Called without the registry lock held -- creating the healer
            // registers its own pool, which takes that lock.
            ensure_running();
            true
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
            entry.owe_repair(now());
            ensure_running();
            true
        }
    }

    /// The interrupt-time counter, as both stamps are measured on.
    ///
    /// `QueryInterruptTime` rather than `QueryPerformanceCounter` because the
    /// only question asked of these values is which of two came first, and this
    /// runs for every callback -- the counter is read from memory the kernel
    /// publishes to user mode, where `QueryPerformanceCounter` may do more.
    /// Reading `KUSER_SHARED_DATA` directly would be the same read and is how
    /// this is often done, but it binds to a layout nothing promises; the
    /// documented call is the specified primitive for the same value.
    ///
    /// # Resolution, and which way its error falls
    ///
    /// The counter advances on the system clock tick -- tens of milliseconds by
    /// default -- so a dispatch and a cancellation within one tick carry equal
    /// stamps. [`PoolEntry::dispatched_since`] compares with `>`, so equal
    /// stamps mean "no dispatch since", and the repair is submitted. That is the
    /// direction the error has to fall: a redundant repair costs one work
    /// submission, where a suppressed one leaves a pool stalled. The opposite
    /// mistake cannot happen at any resolution, because the counter never goes
    /// backwards, so a dispatch stamp can never exceed a cancellation that
    /// followed it.
    pub(crate) fn now() -> u64 {
        let mut ticks = 0_u64;
        // SAFETY: the out-parameter is a live local for the duration of the call.
        unsafe { QueryInterruptTime(&raw mut ticks) };
        ticks
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
        let Some(repair) = create_repair(key) else {
            crate::trace_record!("heal", "register-failed", key);
            return Registration { key, entry: None };
        };
        let entry = Arc::new(PoolEntry {
            key,
            last_dispatch: AtomicU64::new(0),
            repair_owed_at: AtomicU64::new(0),
            objects: AtomicUsize::new(1),
            repair,
        });
        crate::trace_record!("heal", "entry-created", key, entry.repair.work);
        entries.push(Arc::clone(&entry));
        Registration {
            key,
            entry: Some(entry),
        }
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
            if remaining > 0 || entry.repair_owed_at().is_some() {
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
    /// `RepairWork::drop`, which drains the work object -- and that drain cannot
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
        // Dropping the last `Arc` in place would run `RepairWork::drop`, which
        // now drains the work object before closing it -- and that drain waits
        // on a pool this feature only touches because it is suspected of not
        // dispatching. Holding the process-wide registry lock across such a
        // wait would let one wedged pool stall every other pool's registration
        // and release.
        let retired: Vec<Arc<PoolEntry>> = {
            let mut entries = locked();
            let (out, keep) = entries.iter().cloned().partition(|entry| {
                let idle = entry.objects.load(Ordering::Relaxed) == 0
                    && entry.repair_owed_at().is_none()
                    && entry.repair_settled();
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
    fn ensure_running() {
        /// Fast path, so the common already-running case pays a load rather
        /// than a lock on the cancellation path.
        static RUNNING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        static HEALER: Mutex<Option<Healer>> = Mutex::new(None);

        if RUNNING.load(Ordering::Acquire) {
            return;
        }
        // Held across construction, which registers the healer's own pool and
        // so takes the registry lock. That order -- this lock, then the
        // registry -- is the only one taken anywhere: `tick` and `register`
        // take the registry lock and never this one, and `owe_repair` calls
        // here without holding it.
        let mut slot = HEALER.lock().unwrap_or_else(|poison| poison.into_inner());
        if slot.is_some() {
            return;
        }
        let Some(healer) = build() else {
            crate::trace_record!("heal", "healer-start-failed", 0);
            return;
        };
        crate::trace_record!("heal", "healer-started", 0);
        *slot = Some(healer);
        RUNNING.store(true, Ordering::Release);
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

    /// [`tick`] without taking the test gate.
    pub(crate) fn tick_inner() {
        for entry in entries() {
            let Some(owed_at) = entry.repair_owed_at() else {
                continue;
            };
            if entry.dispatched_since(owed_at) {
                // Direct evidence the pool is still delivering callbacks, so
                // whatever the cancellation may have done, it did not stop it.
                crate::trace_record!("heal", "repair-unnecessary", entry.key());
                entry.clear_repair();
                continue;
            }
            // The one action measured to release the stall every time. The work
            // object was made when the pool was registered, so this allocates
            // nothing and makes one call -- which matters because the pool it is
            // aimed at may already be wedged.
            crate::trace_record!("heal", "repair-submitted", entry.key(), entry.repair_work());
            // Counted *before* the submit, never after: the repair can be
            // dispatched on a pool thread the instant it is handed over, so a
            // count taken afterwards can be overtaken by the run it is meant to
            // be paired against, and `repair_settled` would then read `true`
            // with a submission outstanding -- which is the one answer it must
            // never give.
            entry.repair_submitted();
            // SAFETY: the work object was created by `create_repair` for this
            // entry and is alive while the entry is, which this `Arc` ensures.
            unsafe { SubmitThreadpoolWork(entry.repair_work()) };
            entry.clear_repair();
        }
        // Entries kept alive only by an owed repair become retirable once it is
        // discharged, and this is the only place that can notice.
        retire_idle();
    }

    /// Force [`create_repair`] to fail for one pool, so the untracked path can
    /// be tested.
    ///
    /// That path is reached only when `CreateThreadpoolWork` fails, which means
    /// the process is out of memory -- not a condition a test can produce on
    /// demand, and the error edge would otherwise be written but never
    /// executed. This makes it reachable deterministically.
    ///
    /// **Keyed to a single pool, not a plain on/off flag.** The registry is
    /// process-wide and these tests run as threads in one process, so a boolean
    /// here fails registrations belonging to whichever unrelated test happens
    /// to be creating an object at the time. Measured: it did exactly that on
    /// the first run, failing `a_dispatch_after_the_cancellation_is_what_counts`
    /// rather than anything it had to do with. Zero means no pool is forced.
    #[cfg(test)]
    pub(crate) static FORCE_REPAIR_FAILURE_FOR: AtomicUsize = AtomicUsize::new(0);

    /// Make the work object a repair for this pool will submit.
    fn create_repair(key: PoolKey) -> Option<RepairWork> {
        #[cfg(test)]
        if key != 0 && FORCE_REPAIR_FAILURE_FOR.load(Ordering::SeqCst) == key {
            return None;
        }
        // Allocated before the work object, so its address can be the context.
        let runs = Box::new(AtomicU64::new(0));
        let context: *mut core::ffi::c_void = std::ptr::from_ref(runs.as_ref())
            .cast::<core::ffi::c_void>()
            .cast_mut();
        let mut env = crate::callback_env::CallbackEnviron::new();
        // SAFETY: a non-zero key names a live pool -- the caller is creating an
        // object against it in this call -- and the environment is used only for
        // the `CreateThreadpoolWork` below, which copies it.
        unsafe { env.set_pool_raw(key as isize) };
        let work = crate::trace_call!("CreateThreadpoolWork", key, 0, {
            // SAFETY: the trampoline matches the required ABI, the context is
            // the `runs` box moved into the returned value below, and the
            // environment is live for this call.
            unsafe { CreateThreadpoolWork(Some(repair_trampoline), context, env.as_mut_ptr()) }
        });
        // `PTP_WORK` is an opaque handle rather than a pointer, so failure is a
        // zero value, not a null one.
        if work == 0 {
            None
        } else {
            Some(RepairWork {
                work,
                runs,
                submitted: AtomicU64::new(0),
            })
        }
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
    doc = "```compile_fail",
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
