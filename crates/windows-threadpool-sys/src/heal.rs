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
pub(crate) use on::{PoolEntry, Registration, entries, now, register, retire_idle};

#[cfg(not(feature = "self-heal"))]
pub(crate) use off::{Registration, register};

#[cfg(not(feature = "self-heal"))]
mod off {
    use super::PoolKey;

    /// A registration in a build with no registry: nothing, costing nothing.
    pub(crate) struct Registration;

    impl Registration {
        /// Records nothing: with no entry there is no slot to stamp.
        pub(crate) const fn stamp_dispatch(&self) {}
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
    use windows_sys::Win32::System::Threading::{
        CloseThreadpoolWork, CreateThreadpoolWork, PTP_CALLBACK_INSTANCE, PTP_WORK,
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
            self.repair.0
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
    struct RepairWork(PTP_WORK);

    // SAFETY: a PTP_WORK is a pool object the thread pool itself uses across
    // threads; this wrapper only stores it, submits it, and closes it once.
    unsafe impl Send for RepairWork {}
    unsafe impl Sync for RepairWork {}

    impl Drop for RepairWork {
        fn drop(&mut self) {
            crate::trace_record!("heal", "repair-closed", self.0);
            // SAFETY: created by `CreateThreadpoolWork` here, never submitted
            // concurrently with this drop (the entry is unreachable), and closed
            // exactly once.
            unsafe { CloseThreadpoolWork(self.0) };
        }
    }

    /// The repair item's callback, which deliberately does nothing.
    ///
    /// The repair is the *submission*, not the work: a submit is what releases a
    /// stalled pool, and the callback running is only the evidence that it did.
    unsafe extern "system" fn repair_trampoline(
        _instance: PTP_CALLBACK_INSTANCE,
        _context: *mut core::ffi::c_void,
        _work: PTP_WORK,
    ) {
        crate::trace_record!("heal", "repair-ran", _work);
    }

    /// One object's claim on a pool's entry, released when the object goes.
    pub(crate) struct Registration(Option<Arc<PoolEntry>>);

    impl Registration {
        /// The entry, when the pool could be registered.
        pub(crate) fn entry(&self) -> Option<&Arc<PoolEntry>> {
            self.0.as_ref()
        }

        /// Note that this object's pool has just dispatched a callback.
        ///
        /// Called from a trampoline, so it is on the path of every callback the
        /// crate delivers: one counter read and one relaxed store.
        pub(crate) fn stamp_dispatch(&self) {
            if let Some(entry) = &self.0 {
                entry.stamp_dispatch(now());
            }
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
            if let Some(entry) = self.0.take() {
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
    /// into a failure of the caller's actual request. What `try_cancel_pending`
    /// should do when its pool has no entry is `M-T6.3`'s to decide.
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
            return Registration(Some(Arc::clone(entry)));
        }
        let Some(repair) = create_repair(key) else {
            crate::trace_record!("heal", "register-failed", key);
            return Registration(None);
        };
        let entry = Arc::new(PoolEntry {
            key,
            last_dispatch: AtomicU64::new(0),
            repair_owed_at: AtomicU64::new(0),
            objects: AtomicUsize::new(1),
            repair,
        });
        crate::trace_record!("heal", "entry-created", key, entry.repair.0);
        entries.push(Arc::clone(&entry));
        Registration(Some(entry))
    }

    /// Give up one object's claim, retiring the entry when nothing needs it.
    ///
    /// **An entry outlives its objects while a repair is owed**, and that single
    /// rule covers the default pool and a private one alike -- see the decision
    /// recorded with `M-T6.1`. Retiring an entry that still owes a repair would
    /// drop the repair at exactly the moment it is needed.
    fn release(entry: &Arc<PoolEntry>) {
        let mut entries = locked();
        let remaining = entry.objects.fetch_sub(1, Ordering::Relaxed) - 1;
        if remaining > 0 || entry.repair_owed_at().is_some() {
            crate::trace_record!("heal", "released", entry.key, remaining);
            return;
        }
        crate::trace_record!("heal", "entry-retired", entry.key, remaining);
        entries.retain(|held| held.key != entry.key);
    }

    /// Retire every entry that has no objects and owes no repair.
    ///
    /// For the self-heal timer (`M-T6.4`), which is what discharges a repair and
    /// so is what makes a retained entry retirable again.
    pub(crate) fn retire_idle() {
        let mut entries = locked();
        entries.retain(|entry| {
            let keep =
                entry.objects.load(Ordering::Relaxed) > 0 || entry.repair_owed_at().is_some();
            if !keep {
                crate::trace_record!("heal", "entry-retired", entry.key, 0);
            }
            keep
        });
    }

    /// Every entry currently registered.
    ///
    /// For the self-heal timer, and for tests. Clones the `Arc`s so the lock is
    /// not held while a caller works through them -- a repair submits to a pool
    /// that may be wedged, which must not be done holding a process-wide lock.
    pub(crate) fn entries() -> Vec<Arc<PoolEntry>> {
        locked().clone()
    }

    /// Make the work object a repair for this pool will submit.
    fn create_repair(key: PoolKey) -> Option<RepairWork> {
        let mut env = crate::callback_env::CallbackEnviron::new();
        // SAFETY: a non-zero key names a live pool -- the caller is creating an
        // object against it in this call -- and the environment is used only for
        // the `CreateThreadpoolWork` below, which copies it.
        unsafe { env.set_pool_raw(key as isize) };
        let work = crate::trace_call!("CreateThreadpoolWork", key, 0, {
            // SAFETY: the trampoline matches the required ABI, the context is
            // null and never dereferenced, and the environment is live for this
            // call.
            unsafe {
                CreateThreadpoolWork(
                    Some(repair_trampoline),
                    core::ptr::null_mut(),
                    env.as_mut_ptr(),
                )
            }
        });
        // `PTP_WORK` is an opaque handle rather than a pointer, so failure is a
        // zero value, not a null one.
        if work == 0 {
            None
        } else {
            Some(RepairWork(work))
        }
    }
}

#[cfg(test)]
mod tests;
