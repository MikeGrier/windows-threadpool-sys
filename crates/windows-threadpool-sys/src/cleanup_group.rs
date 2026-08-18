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
//! group.close_members(false);
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
use std::os::windows::io::{AsHandle, BorrowedHandle, OwnedHandle};
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
use crate::wait::{ThreadpoolWait, WaitActivation, WaitableHandle};
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
    free: unsafe fn(*mut c_void),
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
/// group.close_members(false);
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
        let group = unsafe { CreateThreadpoolCleanupGroup() };
        if group == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            group,
            resources: Mutex::new(Vec::new()),
        })
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
            free: ThreadpoolTimer::drop_context,
        });
        Ok(TimerMember {
            handle,
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
        let (raw, context, handle) = wait.into_parts();
        self.adopt(OwnedResource {
            ptr: context,
            prepare_shutdown: ThreadpoolWait::prepare_shutdown,
            free: ThreadpoolWait::drop_context,
        });
        // The handle outlives the member for the same reason the context does.
        let handle = Box::into_raw(Box::new(handle));
        self.adopt(OwnedResource {
            ptr: handle.cast(),
            prepare_shutdown: prepare_shutdown_noop,
            free: free_boxed::<OwnedHandle>,
        });
        Ok(WaitMember {
            handle: raw,
            watched: handle,
            _group: PhantomData,
        })
    }

    /// Release every member of this group.
    ///
    /// Waits for executing callbacks to finish. When `cancel_pending` is true,
    /// callbacks that have not started are dropped instead of run; when false,
    /// they run first.
    ///
    /// Taking `&mut self` is what makes members unusable afterwards: they borrow
    /// the group, so the compiler rejects any later use of one. Calling this
    /// twice is harmless -- the second call finds no members.
    ///
    /// The group remains usable afterwards. New members may be created on it,
    /// and they are released by the next call or by `Drop`, exactly as the first
    /// batch was.
    pub fn close_members(&mut self, cancel_pending: bool) {
        self.release_members(cancel_pending);
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

        // SAFETY: the group is live. This waits for executing callbacks and
        // releases every member, so afterwards nothing can reach the contexts.
        unsafe {
            CloseThreadpoolCleanupGroupMembers(
                self.group,
                if cancel_pending { TRUE } else { FALSE },
                ptr::null_mut(),
            );
        }

        let resources = std::mem::take(
            &mut *self
                .resources
                .lock()
                .unwrap_or_else(|poison| poison.into_inner()),
        );
        for resource in resources {
            // SAFETY: every member has been released, so no callback can still
            // reach this allocation; each is freed exactly once here.
            unsafe { (resource.free)(resource.ptr) };
        }
    }
}

impl Drop for CleanupGroup {
    fn drop(&mut self) {
        // Let queued callbacks run, matching the default of `close_members`.
        self.release_members(false);
        // SAFETY: the members are released, so the group can be closed.
        unsafe { CloseThreadpoolCleanupGroup(self.group) };
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
        unsafe { SubmitThreadpoolWork(self.handle) };
    }

    /// Block until all queued and in-progress invocations have completed.
    pub fn wait(&self) {
        // SAFETY: as above.
        unsafe { WaitForThreadpoolWorkCallbacks(self.handle, FALSE) };
    }

    /// Cancel invocations that have not started, then wait for those that have.
    pub fn cancel_pending(&self) {
        // SAFETY: as above.
        unsafe { WaitForThreadpoolWorkCallbacks(self.handle, TRUE) };
    }
}

/// A one-shot timer owned by a [`CleanupGroup`].
///
/// Behaves like [`ThreadpoolTimer`] but is released by the group rather than by
/// its own drop.
#[derive(Debug)]
pub struct TimerMember<'group> {
    handle: PTP_TIMER,
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
        unsafe { IsThreadpoolTimerSet(self.handle) != 0 }
    }

    /// Block until all queued and executing callbacks have completed.
    pub fn wait(&self) {
        // SAFETY: as above.
        unsafe { WaitForThreadpoolTimerCallbacks(self.handle, FALSE) };
    }

    /// Cancel callbacks that have not started, then wait for those that have.
    pub fn cancel_pending(&self) {
        // SAFETY: as above.
        unsafe { WaitForThreadpoolTimerCallbacks(self.handle, TRUE) };
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
        unsafe { IsThreadpoolTimerSet(self.handle) != 0 }
    }

    /// Block until all queued and executing ticks have completed.
    pub fn wait(&self) {
        // SAFETY: as above.
        unsafe { WaitForThreadpoolTimerCallbacks(self.handle, FALSE) };
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
        unsafe { WaitForThreadpoolTimerCallbacks(self.handle, TRUE) };
    }
}

/// A wait object owned by a [`CleanupGroup`].
///
/// Behaves like [`ThreadpoolWait`] but is released by the group rather than by
/// its own drop, and the watched handle is owned by the group.
#[derive(Debug)]
pub struct WaitMember<'group> {
    handle: PTP_WAIT,
    watched: *mut OwnedHandle,
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
        // SAFETY: the handle is owned by the group, which outlives this member.
        unsafe { (*self.watched).as_handle() }
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
        unsafe { WaitForThreadpoolWaitCallbacks(self.handle, FALSE) };
    }

    /// Cancel callbacks that have not started, then wait for those that have.
    pub fn cancel_pending(&self) {
        // SAFETY: as above.
        unsafe { WaitForThreadpoolWaitCallbacks(self.handle, TRUE) };
    }
}

#[cfg(test)]
mod tests;
