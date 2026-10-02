// Copyright (c) 2026 Mike Grier
//! Cleanup groups: releasing many thread-pool objects in one step.
//!
//! A cleanup group tears down every object created into it with a single
//! `CloseThreadpoolCleanupGroupMembers`, which waits for executing callbacks and
//! (optionally) cancels those that have not started. That is the SDK's answer to
//! shutting down a subsystem without tracking each object individually.
//!
//! # Why members are created *by* the group
//!
//! Releasing members is bulk and irreversible: afterwards a member must not be
//! used or closed again, and only then is its heap callback context safe to
//! free. An individually-owned object cannot know when that has happened, so the
//! group owns both the members and their contexts.
//!
//! That ownership is expressed in the types. Members borrow the group, and
//! [`CleanupGroup::close_members`] takes `&mut self`, so the borrow checker
//! rejects any use of a member after the group has released it:
//!
//! ```compile_fail
//! # use windows_threadpool_sys::cleanup_group::CleanupGroup;
//! let mut group = CleanupGroup::new().expect("create group");
//! let work = group.create_work(|| {}, None).expect("create work");
//! group.close_members();
//! work.submit(); // error: `group` is mutably borrowed above
//! ```
//!
//! # Thread-pool I/O is deliberately excluded
//!
//! There is no `create_io`. A `TP_IO` object must not be closed while any
//! overlapped operation is outstanding, because the kernel still owns that
//! operation's storage -- and a cleanup group's bulk release has no way to
//! satisfy that precondition for its members. [`crate::io::ThreadpoolIo`]
//! therefore stays individually owned, where its `Drop` can cancel, drain, and
//! only then close. Grouping it would trade a guarantee for a convenience.

use core::ffi::c_void;
use std::io;
use std::marker::PhantomData;
use std::os::windows::io::BorrowedHandle;
use std::ptr;
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use windows_sys::Win32::Foundation::{FALSE, TRUE};
use windows_sys::Win32::System::Threading::{
    CloseThreadpoolCleanupGroup, CloseThreadpoolCleanupGroupMembers, CreateThreadpoolCleanupGroup,
    IsThreadpoolTimerSet, PTP_CLEANUP_GROUP, PTP_TIMER, PTP_WAIT, PTP_WORK, SubmitThreadpoolWork,
    WaitForThreadpoolTimerCallbacks, WaitForThreadpoolWaitCallbacks,
    WaitForThreadpoolWorkCallbacks,
};

use crate::callback_env::CallbackEnviron;
use crate::timer::{
    PeriodicTick, ThreadpoolPeriodicTimer, ThreadpoolTimer, TimerFiring, absolute_filetime,
    arm_raw, disarm_raw, millis_u32, relative_filetime,
};
use crate::wait::{ThreadpoolWait, WaitActivation, WaitTarget, WaitableHandle};
use crate::work::ThreadpoolWork;

/// A heap allocation the group frees once its members have been released.
///
/// Resources are type-erased because one group holds members of several kinds;
/// each entry carries the function that knows how to free it, and the function
/// that prepares its member for the bulk release.
struct OwnedResource {
    ptr: *mut c_void,
    /// Suppress the member's deferred re-arm and disarm it before
    /// `CloseThreadpoolCleanupGroupMembers` runs. A no-op for kinds with no
    /// callback-driven re-arm (work, periodic timers, watched handles).
    prepare_shutdown: unsafe fn(*mut c_void),
    /// Mark the member's pool as owing a self-heal repair, when the release is
    /// a cancelling one. A no-op for every kind but a wait: the removal that
    /// can sever a pool's arrival notification operates on a wait completion
    /// packet, and only a wait owns one.
    owe_repair: unsafe fn(*mut c_void) -> bool,
    free: unsafe fn(*mut c_void),
}

/// A repair hook for a member whose release cannot wedge a pool.
///
/// SAFETY: takes a pointer it never dereferences.
unsafe fn no_repair_owed(_ptr: *mut c_void) -> bool {
    // Nothing was cancelled on a pool this crate tracks, so there is nothing
    // left untracked. Reporting `true` keeps the untracked fail-fast measuring
    // only the members that can actually owe a repair.
    true
}

// SAFETY: each pointer is a `Box` the group exclusively owns and frees exactly
// once, after the pool has released every member that could reach it.
unsafe impl Send for OwnedResource {}

/// Free a boxed value the group owns directly, rather than a callback context.
///
/// SAFETY: `ptr` must be a `Box<T>` reclaimed exactly once.
unsafe fn free_boxed<T>(ptr: *mut c_void) {
    // SAFETY: forwarded from this function's own contract.
    drop(unsafe { Box::from_raw(ptr.cast::<T>()) });
}

/// A shutdown preparation for a member with no callback-driven re-arm to
/// suppress: work objects, periodic timers, and watched handles.
///
/// `CloseThreadpoolCleanupGroupMembers` already disarms and cancels these; only
/// a one-shot timer or a wait can re-arm itself from inside a callback, so only
/// those need the real preparation.
fn prepare_shutdown_noop(_ptr: *mut c_void) {}

/// An owned thread-pool cleanup group.
///
/// Create members with [`CleanupGroup::create_work`],
/// [`CleanupGroup::create_timer`], [`CleanupGroup::create_periodic_timer`], and
/// [`CleanupGroup::create_wait`], then release them all with
/// [`CleanupGroup::close_members`]. `Drop` releases any members that are still
/// open, so forgetting to call `close_members` is safe -- it only gives up
/// control over *when* the teardown blocks.
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
/// use std::sync::atomic::{AtomicUsize, Ordering};
/// use std::time::Duration;
/// use windows_threadpool_sys::cleanup_group::CleanupGroup;
///
/// let count = Arc::new(AtomicUsize::new(0));
/// let work_counter = Arc::clone(&count);
/// let timer_counter = Arc::clone(&count);
///
/// let mut group = CleanupGroup::new()?;
/// {
///     let work = group.create_work(move || {
///         work_counter.fetch_add(1, Ordering::SeqCst);
///     }, None)?;
///     let timer = group.create_timer(move |_firing| {
///         timer_counter.fetch_add(1, Ordering::SeqCst);
///     }, None)?;
///
///     work.submit();
///     timer.set_after(Duration::from_millis(1));
///
///     // Wait for the work to have run and the timer to have fired. Note that
///     // `timer.wait()` would not do: it waits for callbacks the pool has
///     // already queued, and a timer that has not expired yet has none.
///     while count.load(Ordering::SeqCst) < 2 {
///         std::thread::yield_now();
///     }
/// }
///
/// // One call tears down every member of the group.
/// group.close_members();
/// assert_eq!(count.load(Ordering::SeqCst), 2);
/// # Ok::<(), std::io::Error>(())
/// ```
pub struct CleanupGroup {
    group: PTP_CLEANUP_GROUP,
    /// Contexts and handles owned on behalf of members, freed after release.
    ///
    /// This is the only record of what is outstanding, and it is deliberately
    /// not paired with a "already released" flag. Such a flag would latch: the
    /// `create_*` methods take `&self`, so members can be created after a
    /// release returns, and a latched release would then skip them -- leaking
    /// their contexts and closing the group with live members.
    resources: Mutex<Vec<OwnedResource>>,
    /// Test-only: run between preparing the members and releasing them.
    ///
    /// Stands in for the self-heal's tick arriving in that interval, which is
    /// the only way to observe *when* a cancelling release marks its pools.
    /// Both orderings leave a mark outstanding once the release has returned,
    /// so an end-state assertion cannot tell them apart; something has to act
    /// inside the window. Per-group rather than a static, because `cargo test`
    /// runs these as threads of one process.
    #[cfg(test)]
    before_release: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
}

// SAFETY: PTP_CLEANUP_GROUP is a kernel-managed object usable from any thread,
// and the resource list is mutex-guarded.
unsafe impl Send for CleanupGroup {}
unsafe impl Sync for CleanupGroup {}

impl CleanupGroup {
    /// Create an empty cleanup group.
    ///
    /// # Errors
    ///
    /// Returns the error from `CreateThreadpoolCleanupGroup`.
    pub fn new() -> io::Result<Self> {
        // SAFETY: the call takes no inputs.
        let group = crate::trace_call!("CreateThreadpoolCleanupGroup", 0, 0, {
            // SAFETY: the call takes no inputs.
            unsafe { CreateThreadpoolCleanupGroup() }
        });
        if group == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            group,
            resources: Mutex::new(Vec::new()),
            #[cfg(test)]
            before_release: Mutex::new(None),
        })
    }

    /// Install the test-only hook that runs just before the native release.
    ///
    /// See the field's documentation for why observing that instant is the
    /// only way to pin the order in which a cancelling release marks its
    /// pools.
    ///
    /// Carries the feature condition of its only caller, which is the test for
    /// that marking order: without `self-heal` there is no mark to observe, so
    /// an ungated hook here is dead code in a `--no-default-features` build --
    /// which CI compiles with `-D warnings`.
    #[cfg(all(test, feature = "self-heal"))]
    pub(crate) fn on_before_release(&self, hook: impl Fn() + Send + Sync + 'static) {
        *self
            .before_release
            .lock()
            .unwrap_or_else(|poison| poison.into_inner()) = Some(Box::new(hook));
    }

    /// Build the environment a member is created with, layering this group on
    /// top of whatever pool and priority the caller chose.
    ///
    /// The caller's environment is copied rather than mutated, so passing one
    /// environment to several groups -- or reusing it for a non-member object --
    /// behaves as written.
    fn member_environment(&self, env: Option<&CallbackEnviron<'_>>) -> CallbackEnviron<'_> {
        let mut member_env = match env {
            Some(env) => CallbackEnviron::from_inner(*env.as_inner()),
            None => CallbackEnviron::new(),
        };
        // SAFETY: `self.group` is live for at least as long as the member being
        // created, because the member borrows this group, and the member is
        // never closed individually -- `close_members` releases it.
        unsafe { member_env.set_cleanup_group(self.group, None) };
        member_env
    }

    fn adopt(&self, resource: OwnedResource) {
        self.resources
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .push(resource);
    }

    /// Create a work object owned by this group.
    ///
    /// Equivalent to [`ThreadpoolWork::new`], except that the returned member is
    /// released by [`CleanupGroup::close_members`] rather than by its own drop.
    ///
    /// # Errors
    ///
    /// Returns the error from `CreateThreadpoolWork`.
    pub fn create_work<F>(
        &self,
        callback: F,
        env: Option<&CallbackEnviron>,
    ) -> io::Result<WorkMember<'_>>
    where
        F: Fn() + Send + Sync + 'static,
    {
        let mut member_env = self.member_environment(env);
        let work = ThreadpoolWork::new(callback, Some(&mut member_env))?;
        let (handle, context) = work.into_parts();
        self.adopt(OwnedResource {
            ptr: context,
            prepare_shutdown: prepare_shutdown_noop,
            owe_repair: no_repair_owed,
            free: ThreadpoolWork::drop_context,
        });
        Ok(WorkMember {
            handle,
            _group: PhantomData,
        })
    }

    /// Create a one-shot timer owned by this group.
    ///
    /// Equivalent to [`ThreadpoolTimer::new`].
    ///
    /// # Errors
    ///
    /// Returns the error from `CreateThreadpoolTimer`.
    pub fn create_timer<F>(
        &self,
        callback: F,
        env: Option<&CallbackEnviron>,
    ) -> io::Result<TimerMember<'_>>
    where
        F: Fn(&TimerFiring<'_>) + Send + Sync + 'static,
    {
        let mut member_env = self.member_environment(env);
        let timer = ThreadpoolTimer::new(callback, Some(&mut member_env))?;
        let (handle, context) = timer.into_parts();
        self.adopt(OwnedResource {
            ptr: context,
            prepare_shutdown: ThreadpoolTimer::prepare_shutdown,
            owe_repair: no_repair_owed,
            free: ThreadpoolTimer::drop_context,
        });
        Ok(TimerMember {
            handle,
            context,
            _group: PhantomData,
        })
    }

    /// Create a periodic timer owned by this group.
    ///
    /// Equivalent to [`ThreadpoolPeriodicTimer::new`], including that its ticks
    /// may overlap one another.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::InvalidInput`] if `period` is outside
    /// [`ThreadpoolPeriodicTimer::MIN_PERIOD`]..=[`ThreadpoolPeriodicTimer::MAX_PERIOD`]
    /// or is not a whole number of milliseconds, or the error from
    /// `CreateThreadpoolTimer`.
    pub fn create_periodic_timer<F>(
        &self,
        period: Duration,
        callback: F,
        env: Option<&CallbackEnviron>,
    ) -> io::Result<PeriodicTimerMember<'_>>
    where
        F: Fn(&PeriodicTick<'_>) + Send + Sync + 'static,
    {
        let mut member_env = self.member_environment(env);
        let timer = ThreadpoolPeriodicTimer::new(period, callback, Some(&mut member_env))?;
        let (handle, context, period) = timer.into_parts();
        self.adopt(OwnedResource {
            ptr: context,
            prepare_shutdown: prepare_shutdown_noop,
            owe_repair: no_repair_owed,
            free: ThreadpoolPeriodicTimer::drop_context,
        });
        Ok(PeriodicTimerMember {
            handle,
            period,
            _group: PhantomData,
        })
    }

    /// Create a wait object owned by this group, watching `handle`.
    ///
    /// The group takes ownership of the handle as well as the object, because
    /// the pool may still be watching it until the members are released.
    ///
    /// Like [`ThreadpoolWait::new`], this takes a [`WaitableHandle`] rather than
    /// a bare handle, so the group path cannot reach the unsupported wait
    /// targets that the individually-owned path rejects.
    ///
    /// # Errors
    ///
    /// Returns the error from `CreateThreadpoolWait`.
    pub fn create_wait<F>(
        &self,
        handle: WaitableHandle,
        callback: F,
        env: Option<&CallbackEnviron<'_>>,
    ) -> io::Result<WaitMember<'_>>
    where
        F: Fn(&WaitActivation<'_>) + Send + Sync + 'static,
    {
        let mut member_env = self.member_environment(env);
        let wait = ThreadpoolWait::new(handle, callback, Some(&mut member_env))?;
        let (raw, context, target) = wait.into_parts();
        self.adopt(OwnedResource {
            ptr: context,
            prepare_shutdown: ThreadpoolWait::prepare_shutdown,
            owe_repair: ThreadpoolWait::owe_repair,
            free: ThreadpoolWait::drop_context,
        });
        // The target outlives the member for the same reason the context does.
        // Freeing the box runs `WaitTarget`'s drop, which closes the handle with
        // whichever routine it was built with -- `CloseHandle` for the default
        // path, the caller's for a custom-close target.
        let target = Box::into_raw(Box::new(target));
        self.adopt(OwnedResource {
            ptr: target.cast(),
            prepare_shutdown: prepare_shutdown_noop,
            owe_repair: no_repair_owed,
            free: free_boxed::<WaitTarget>,
        });
        Ok(WaitMember {
            handle: raw,
            watched: target,
            context,
            _group: PhantomData,
        })
    }

    /// Release every member of this group, letting queued callbacks run.
    ///
    /// Waits for executing callbacks to finish; callbacks that have not started
    /// run before the release completes.
    ///
    /// Taking `&mut self` is what makes members unusable afterwards: they borrow
    /// the group, so the compiler rejects any later use of one. Calling this
    /// twice is harmless -- the second call finds no members.
    ///
    /// The group remains usable afterwards. New members may be created on it,
    /// and they are released by the next call or by `Drop`, exactly as the first
    /// batch was.
    ///
    /// To drop queued callbacks instead of running them, see
    /// `close_members_cancelling` -- named without a link because it does not
    /// exist in a build with `self-heal` off, while this method does, so a link
    /// would dangle in that configuration.
    pub fn close_members(&mut self) {
        self.release_members(false);
    }

    /// Release every member of this group, dropping queued callbacks.
    ///
    /// As [`close_members`](Self::close_members), except that callbacks which
    /// have not started are dropped rather than run.
    ///
    /// A cancelling release passes the cancel through to every member, so a wait
    /// among them reaches the same removal primitive
    /// [`ThreadpoolWait::try_cancel_pending`](crate::wait::ThreadpoolWait::try_cancel_pending)
    /// does, and owes its pool the same repair. This crate marks that repair
    /// here, which is what makes this safe to offer -- subject to the same
    /// stated hole as the per-object method: a member whose pool could not be
    /// registered for repair has that registration retried during this call,
    /// and only if the retry also fails is it cancelled with nothing to repair
    /// it, recording `cancel-untracked` and panicking under `fail-fast`.
    ///
    /// # Availability
    ///
    /// Requires the `self-heal` feature, which is on by default. Without it this
    /// method does not exist and a call to it fails to compile, naming
    /// [`close_members_cancelling_no_heal_tracking`](Self::close_members_cancelling_no_heal_tracking)
    /// as what to reach for instead. That is the designed behaviour, not an
    /// oversight: the repair this method performs is what the feature provides,
    /// and a compile error is the only way a caller relying on it finds out it
    /// is gone.
    #[cfg(feature = "self-heal")]
    pub fn close_members_cancelling(&mut self) {
        // SAFETY: the obligation this transfers is discharged by
        // `release_members`, which marks every member's pool for repair after
        // the native release and before the contexts are freed.
        unsafe { self.close_members_cancelling_no_heal_tracking() };
    }

    /// `close_members_cancelling` without the repair.
    ///
    /// Not a link, deliberately: the method it would name does not exist in a
    /// build with `self-heal` off, and this one does, so the link would dangle
    /// in exactly the configuration this method exists for.
    ///
    /// Always present, including in builds with `self-heal` off, which is the
    /// point: it is the method that still exists when the gated one does not,
    /// and its signature says what the caller takes on.
    ///
    /// # Safety
    ///
    /// The caller must ensure every member's pool is repaired. Submitting any
    /// work item to a pool does so; so does knowing it is kept live by
    /// something else. Leaving one unrepaired can stop that pool dispatching --
    /// which, for the process-default pool, reaches every component in the
    /// process, including code with no connection to this call.
    ///
    /// This is not a memory-safety obligation, and the keyword is not claiming
    /// one. It is here because the obligation is statable and dischargeable by
    /// the caller, which is what `unsafe` marks; see
    /// [the self-heal decision](https://docs.rs/crate/windows-threadpool-sys/latest/source/DESIGN-NOTES.md).
    pub unsafe fn close_members_cancelling_no_heal_tracking(&mut self) {
        self.release_members(true);
    }

    /// The number of contexts and handles the group is holding for its members.
    ///
    /// Zero once the members have been released.
    #[must_use]
    pub fn owned_resources(&self) -> usize {
        self.resources
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .len()
    }

    /// Release whatever members exist right now.
    ///
    /// Runs in full every time rather than latching after the first call. The
    /// native release is idempotent -- with no members it does nothing -- and
    /// running unconditionally is what makes a group usable again afterwards:
    /// members created after an earlier release are released by the next one,
    /// instead of being skipped and leaked.
    fn release_members(&mut self, cancel_pending: bool) {
        // Close the door on any deferred re-arm before the bulk release.
        // `CloseThreadpoolCleanupGroupMembers` waits for executing callbacks but
        // does not stop one from re-arming: a one-shot timer or wait whose
        // callback is running can request a re-arm the trampoline applies after
        // it returns, which would re-arm an object the release is tearing down
        // and then free its context under a freshly queued callback. Suppressing
        // and disarming each member first mirrors what each object's own `Drop`
        // does. The lock is dropped before the release, which blocks.
        {
            let resources = self
                .resources
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            for resource in resources.iter() {
                // SAFETY: the members are still live and unreleased; each hook
                // matches the context kind this resource holds and only
                // suppresses/disarms that one object.
                unsafe { (resource.prepare_shutdown)(resource.ptr) };
            }
        }

        #[cfg(test)]
        {
            let hook = self
                .before_release
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            if let Some(hook) = hook.as_ref() {
                hook();
            }
        }

        // The longest-blocking call in this crate: it waits for every member's
        // executing callback. Bracketed so a release that parks is visible as
        // an interval rather than inferred from the gap after it.
        crate::trace_call!(
            "CloseThreadpoolCleanupGroupMembers",
            self.group,
            u32::from(cancel_pending),
            {
                // SAFETY: the group is live. This waits for executing callbacks and
                // releases every member, so afterwards nothing can reach the contexts.
                unsafe {
                    CloseThreadpoolCleanupGroupMembers(
                        self.group,
                        if cancel_pending { TRUE } else { FALSE },
                        ptr::null_mut(),
                    );
                }
            }
        );

        let resources = std::mem::take(
            &mut *self
                .resources
                .lock()
                .unwrap_or_else(|poison| poison.into_inner()),
        );
        let mut tracked = true;
        if cancel_pending {
            for resource in resources.iter() {
                // A cancelling release passes the cancel through to every
                // member, so each wait among them reaches the same removal
                // `ThreadpoolWait::try_cancel_pending` does and owes its pool
                // the same repair.
                //
                // Marked **after** the release returns, matching the standalone
                // cancel path, and before the contexts are freed below -- which
                // is why this loop sits between the two rather than beside
                // either.
                //
                // An earlier version marked before the release, justified by
                // the claim that the native call frees the contexts. It does
                // not: this function frees them, in the loop immediately after
                // this one. Marking first left a window in which the healer
                // could see the mark, find the pool still dispatching, clear it
                // as repaired, and then have the real cancellation happen with
                // no mark outstanding -- an unrepaired wedge from a *single*
                // cancellation, where the race this crate already documents
                // needs two.
                //
                // SAFETY: the contexts are still alive -- nothing is freed
                // until the loop below -- and each hook matches the context
                // kind this resource holds.
                //
                // Accumulated rather than acted on here: the fail-fast this
                // feeds panics, and a panic raised in this loop would unwind
                // past the free loop below and leak every member's context.
                tracked &= unsafe { (resource.owe_repair)(resource.ptr) };
            }
        }
        for resource in resources {
            // SAFETY: every member has been released, so no callback can still
            // reach this allocation; each is freed exactly once here.
            unsafe { (resource.free)(resource.ptr) };
        }
        // After the frees, never before: this panics under `fail-fast`, and an
        // unwind from inside either loop above would skip the frees.
        crate::obligation::fail_fast_if_untracked(tracked, "CleanupGroup");
    }
}

impl Drop for CleanupGroup {
    fn drop(&mut self) {
        // Let queued callbacks run, matching `close_members`.
        self.release_members(false);
        // SAFETY: the members are released, so the group can be closed.
        crate::trace_call!("CloseThreadpoolCleanupGroup", self.group, 0, {
            // SAFETY: the group is live and closed exactly once, here.
            unsafe { CloseThreadpoolCleanupGroup(self.group) };
        });
    }
}

impl std::fmt::Debug for CleanupGroup {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CleanupGroup")
            .field("owned_resources", &self.owned_resources())
            .finish_non_exhaustive()
    }
}

/// A work object owned by a [`CleanupGroup`].
///
/// Behaves like [`ThreadpoolWork`] but is released by the group rather than by
/// its own drop.
#[derive(Debug)]
pub struct WorkMember<'group> {
    handle: PTP_WORK,
    _group: PhantomData<&'group CleanupGroup>,
}

impl WorkMember<'_> {
    /// Queue one invocation of the callback.
    pub fn submit(&self) {
        // SAFETY: the handle is live until the group releases its members,
        // which the borrow on `_group` prevents from happening first.
        crate::trace_call!("SubmitThreadpoolWork", self.handle, 0, {
            // SAFETY: the handle is live until the group releases it.
            unsafe { SubmitThreadpoolWork(self.handle) };
        });
    }

    /// Block until all queued and in-progress invocations have completed.
    pub fn wait(&self) {
        // SAFETY: as above.
        crate::trace_call!("WaitForThreadpoolWorkCallbacks", self.handle, 0, {
            // SAFETY: the handle is live until the group releases it.
            unsafe { WaitForThreadpoolWorkCallbacks(self.handle, FALSE) };
        });
    }

    /// Stop accepting work and block until none is queued or executing.
    ///
    /// As on [`ThreadpoolWork`], there is nothing to *stop* -- a submission
    /// cannot be withdrawn, only waited for -- so this is exactly
    /// [`wait`](Self::wait). The name exists so a caller tearing down a mixed
    /// set of objects can reach for one method.
    pub fn stop_and_drain(&self) {
        self.wait();
    }

    /// Cancel invocations that have not started, then wait for those that have.
    pub fn cancel_pending(&self) {
        // SAFETY: as above.
        crate::trace_call!("WaitForThreadpoolWorkCallbacks(cancel)", self.handle, 1, {
            // SAFETY: the handle is live until the group releases it.
            unsafe { WaitForThreadpoolWorkCallbacks(self.handle, TRUE) };
        });
    }
}

/// A one-shot timer owned by a [`CleanupGroup`].
///
/// Behaves like [`ThreadpoolTimer`] but is released by the group rather than by
/// its own drop.
#[derive(Debug)]
pub struct TimerMember<'group> {
    handle: PTP_TIMER,
    /// The callback context the group owns for this member.
    ///
    /// Held so the member can run the same `stop_and_drain` its standalone twin
    /// does, suppression and all. The group owns and frees it; this is a borrow
    /// for the member's lifetime.
    context: *mut c_void,
    _group: PhantomData<&'group CleanupGroup>,
}

impl TimerMember<'_> {
    /// Fire once, `delay` from now.
    pub fn set_after(&self, delay: Duration) {
        // SAFETY: the handle is live until the group releases its members.
        unsafe { arm_raw(self.handle, relative_filetime(delay), 0, 0) };
    }

    /// Fire once at the wall-clock instant `when`.
    pub fn set_at(&self, when: SystemTime) {
        // SAFETY: as above.
        unsafe { arm_raw(self.handle, absolute_filetime(when), 0, 0) };
    }

    /// Stop the timer.
    pub fn disarm(&self) {
        // SAFETY: as above.
        unsafe { disarm_raw(self.handle) };
    }

    /// Whether the timer currently has a due time.
    ///
    /// As with [`ThreadpoolTimer::is_set`], expiring does not clear the due
    /// time; only disarming does.
    #[must_use]
    pub fn is_set(&self) -> bool {
        // SAFETY: as above.
        crate::trace_call!("IsThreadpoolTimerSet", self.handle, 0, {
            // SAFETY: the handle is live until the group releases it.
            unsafe { IsThreadpoolTimerSet(self.handle) != 0 }
        })
    }

    /// Block until all queued and executing callbacks have completed.
    pub fn wait(&self) {
        // SAFETY: as above.
        crate::trace_call!("WaitForThreadpoolTimerCallbacks", self.handle, 0, {
            // SAFETY: the handle is live until the group releases it.
            unsafe { WaitForThreadpoolTimerCallbacks(self.handle, FALSE) };
        });
    }

    /// Stop the timer and block until no firing is queued or executing.
    ///
    /// The same drain [`ThreadpoolTimer::stop_and_drain`] performs, including
    /// suppressing a re-arm a running callback asks for: without that the drain
    /// could return with a due time installed, which is the whole reason the
    /// standalone type has this method rather than only `disarm` and `wait`.
    ///
    /// Added because a member that lacked it was not the equivalent of its
    /// standalone twin that [`CleanupGroup::create_timer`] says it is.
    pub fn stop_and_drain(&self) {
        // SAFETY: the group owns this context and does not free it while this
        // member borrows the group, and `handle` is the object it belongs to.
        unsafe { ThreadpoolTimer::stop_and_drain_parts(self.context, self.handle) };
    }

    /// Cancel callbacks that have not started, then wait for those that have.
    ///
    /// Unlike the wait member's method of the same name, this carries no
    /// process-wide hazard. The primitive that can sever a pool's
    /// arrival-to-factory notification operates on a *wait completion packet*,
    /// which only a wait owns; a timer has none.
    pub fn cancel_pending(&self) {
        // SAFETY: as above.
        crate::trace_call!("WaitForThreadpoolTimerCallbacks(cancel)", self.handle, 1, {
            // SAFETY: the handle is live until the group releases it.
            unsafe { WaitForThreadpoolTimerCallbacks(self.handle, TRUE) };
        });
    }
}

/// A periodic timer owned by a [`CleanupGroup`].
///
/// Behaves like [`ThreadpoolPeriodicTimer`] -- including that its ticks may run
/// concurrently with one another -- but is released by the group rather than by
/// its own drop.
#[derive(Debug)]
pub struct PeriodicTimerMember<'group> {
    handle: PTP_TIMER,
    period: Duration,
    _group: PhantomData<&'group CleanupGroup>,
}

impl PeriodicTimerMember<'_> {
    /// The period this timer ticks on.
    #[must_use]
    pub fn period(&self) -> Duration {
        self.period
    }

    /// Start ticking, with the first tick one period from now.
    pub fn start(&self) {
        self.start_after(self.period);
    }

    /// Start ticking, with the first tick `first_delay` from now.
    pub fn start_after(&self, first_delay: Duration) {
        // SAFETY: the handle is live until the group releases its members.
        unsafe {
            arm_raw(
                self.handle,
                relative_filetime(first_delay),
                millis_u32(self.period),
                0,
            );
        }
    }

    /// Stop the timer.
    pub fn stop(&self) {
        // SAFETY: as above.
        unsafe { disarm_raw(self.handle) };
    }

    /// Whether the timer is currently started.
    #[must_use]
    pub fn is_running(&self) -> bool {
        // SAFETY: as above.
        crate::trace_call!("IsThreadpoolTimerSet", self.handle, 0, {
            // SAFETY: the handle is live until the group releases it.
            unsafe { IsThreadpoolTimerSet(self.handle) != 0 }
        })
    }

    /// Block until all queued and executing ticks have completed.
    pub fn wait(&self) {
        // SAFETY: as above.
        crate::trace_call!("WaitForThreadpoolTimerCallbacks", self.handle, 0, {
            // SAFETY: the handle is live until the group releases it.
            unsafe { WaitForThreadpoolTimerCallbacks(self.handle, FALSE) };
        });
    }

    /// Stop the timer and wait until no tick is queued or executing.
    ///
    /// As with [`ThreadpoolPeriodicTimer::stop_and_drain`], this holds provided
    /// no other thread starts the member during the call: the `start*` methods
    /// take `&self`, so a start landing between the stop and the drain would
    /// leave a schedule installed on return.
    pub fn stop_and_drain(&self) {
        self.stop();
        // SAFETY: as above.
        crate::trace_call!("WaitForThreadpoolTimerCallbacks(cancel)", self.handle, 1, {
            // SAFETY: the handle is live until the group releases it.
            unsafe { WaitForThreadpoolTimerCallbacks(self.handle, TRUE) };
        });
    }
}

/// A wait object owned by a [`CleanupGroup`].
///
/// Behaves like [`ThreadpoolWait`] but is released by the group rather than by
/// its own drop, and the watched handle is owned by the group.
#[derive(Debug)]
pub struct WaitMember<'group> {
    handle: PTP_WAIT,
    watched: *mut WaitTarget,
    /// The callback context the group owns for this member.
    ///
    /// Held so the member can mark its own pool as owing a repair. The group
    /// owns and frees it; this is a borrow for the member's lifetime.
    context: *mut c_void,
    _group: PhantomData<&'group CleanupGroup>,
}

// SAFETY: both pointers refer to state the group owns and outlives this member;
// the member only reads them and passes them to thread-safe pool APIs.
unsafe impl Send for WaitMember<'_> {}
unsafe impl Sync for WaitMember<'_> {}

impl WaitMember<'_> {
    /// Borrow the watched handle, for signalling or inspecting it.
    #[must_use]
    pub fn handle(&self) -> BorrowedHandle<'_> {
        // SAFETY: the target is owned by the group, which outlives this member.
        unsafe { (*self.watched).borrow() }
    }

    /// Arm the wait, so the next signal or timeout runs the callback once.
    pub fn arm(&self, timeout: Option<Duration>) {
        // SAFETY: the object and handle are live until the group releases its
        // members, which the borrow on `_group` prevents from happening first.
        unsafe { crate::wait::arm_member(self.handle, &*self.watched, timeout) };
    }

    /// Stop watching.
    pub fn disarm(&self) {
        // SAFETY: as above.
        unsafe { crate::wait::disarm_raw(self.handle) };
    }

    /// Block until all queued and executing callbacks have completed.
    pub fn wait(&self) {
        // SAFETY: as above.
        crate::trace_call!("WaitForThreadpoolWaitCallbacks", self.handle, 0, {
            // SAFETY: the handle is live until the group releases it.
            unsafe { WaitForThreadpoolWaitCallbacks(self.handle, FALSE) };
        });
    }

    /// Cancel callbacks that have not started, then wait for those that have.
    ///
    /// # This brings a process-wide hazard forward; it does not create it
    ///
    /// Same as
    /// [`ThreadpoolWait::try_cancel_pending_no_heal_tracking`](crate::wait::ThreadpoolWait::try_cancel_pending_no_heal_tracking)
    /// -- see there for the full account. In short: removing an already-delivered
    /// completion packet can permanently sever a pool's arrival-to-factory
    /// notification, but the member's eventual release performs the same removal
    /// anyway, so avoiding this call does not avoid the hazard. Draining does,
    /// because it leaves nothing to remove.
    ///
    /// Owning the wait through a cleanup group does **not** change this. The
    /// group's own `Drop` is safe because it releases with cancel-pending false,
    /// not because the group protects its members; a cancelling release passes
    /// the cancel through to each member. Named without a link, for the reason
    /// `try_cancel_pending_no_heal_tracking` gives: `close_members_cancelling`
    /// does not exist in a build with `self-heal` off, while this method does,
    /// so a link would dangle in that configuration.
    ///
    /// Stop watching and block until no callback is queued or executing.
    ///
    /// The same drain [`ThreadpoolWait::stop_and_drain`] performs, including
    /// suppressing a re-arm a running callback asks for: without that the drain
    /// could return with the object armed again, which is the whole reason the
    /// standalone type has this method rather than only `disarm` and `wait`.
    ///
    /// Added because a member that lacked it was not the equivalent of its
    /// standalone twin that [`CleanupGroup::create_wait`] says it is.
    pub fn stop_and_drain(&self) {
        // SAFETY: the group owns this context and does not free it while this
        // member borrows the group, and `handle` is the object it belongs to.
        unsafe { ThreadpoolWait::stop_and_drain_parts(self.context, self.handle) };
    }

    /// Prefer [`wait`](Self::wait).
    ///
    /// This crate repairs the pool afterwards -- and on a pool whose repair
    /// item could not be created, this call tries to create it rather than
    /// giving up. Only if that fails too does the cancellation proceed with
    /// nothing to repair it, recording `cancel-untracked` and panicking under
    /// `fail-fast`. That is what makes this safe to offer. See
    /// [README-FEATURE-self-heal.md](https://docs.rs/crate/windows-threadpool-sys/latest/source/README-FEATURE-self-heal.md).
    ///
    /// The registry itself is private, so this names it rather than linking it:
    /// a public page linking a private item renders a reference the reader
    /// cannot follow, which rustdoc warns about.
    #[cfg(feature = "self-heal")]
    pub fn try_cancel_pending(&self) {
        // SAFETY: the obligation this transfers is discharged here, by marking
        // the pool so the self-heal repairs it.
        unsafe { self.try_cancel_pending_no_heal_tracking() };
        // SAFETY: the group owns this context and does not free it while this
        // member borrows the group.
        let tracked = unsafe { ThreadpoolWait::owe_repair(self.context) };
        // Last in the function. Unlike the group-wide release this does not
        // free anything, so there is nothing here for an unwind to skip -- but
        // the placement matches the other two call sites rather than relying on
        // that staying true.
        crate::obligation::fail_fast_if_untracked(tracked, "WaitMember");
    }

    /// `try_cancel_pending` without the repair.
    ///
    /// Not a link, deliberately: the method it would name does not exist in a
    /// build with `self-heal` off, and this one does, so the link would dangle
    /// in exactly the configuration this method exists for.
    ///
    /// # Safety
    ///
    /// The caller must ensure the pool is repaired, by submitting any work item
    /// to it or by knowing something else keeps it live. See
    /// [`ThreadpoolWait::try_cancel_pending_no_heal_tracking`], whose obligation
    /// this is.
    pub unsafe fn try_cancel_pending_no_heal_tracking(&self) {
        // SAFETY: as above.
        crate::trace_call!("WaitForThreadpoolWaitCallbacks(cancel)", self.handle, 1, {
            // SAFETY: the handle is live until the group releases it.
            unsafe { WaitForThreadpoolWaitCallbacks(self.handle, TRUE) };
        });
    }
}

#[cfg(test)]
mod tests;
