// Copyright (c) Mike Grier.

//! A queue's readiness, expressed as a waitable Windows `HANDLE`.
//!
//! This is the part of the crate its name refers to. A queue shape owns a
//! [`Doorbell`] and keeps it in agreement with its own emptiness; a client that
//! wants to park on the queue *and* on an I/O completion *and* on a shutdown
//! event in one wait borrows the handle and hands it to
//! `WaitForMultipleObjects` alongside the others.
//!
//! # Manual-reset, and level-triggered
//!
//! The event is manual-reset, so it means "there is something to take" rather
//! than "something arrived". That is the difference between a state and an
//! edge, and only the state composes: `WaitForMultipleObjects` may report any
//! one of several signalled handles, so a waiter routinely learns about one
//! ready source while ignoring another. An auto-reset event consumed by that
//! wait would lose the second source's only edge. A level survives being
//! ignored, and will still be there on the next pass.
//!
//! The crate's own probe made this concrete before the design was fixed: an
//! auto-reset event does not count signals, so two pushes and one wait leave a
//! consumer blocked forever on an item that is sitting in the queue.
//!
//! # Created lazily, so polling is free
//!
//! A consumer that only ever calls `pop` in a loop of its own never needs a
//! kernel object, and should not be charged for one. The event is therefore
//! created on the first request for the handle and not before, following the
//! precedent already set by `windows-file-watcher`'s notification queue.
//!
//! The cost of that laziness is a race worth stating plainly: a producer that
//! runs while no event exists yet skips signalling, because there is nothing to
//! signal. If a consumer could create the doorbell and then immediately wait on
//! it, an item pushed during that window would never wake anyone. What closes
//! the hole is the arming protocol below, not the creation itself -- the
//! doorbell must exist *before* the emptiness check that decides to wait.
//!
//! # The arming protocol, which is the whole correctness argument
//!
//! [`Doorbell`] cannot enforce this itself, because it cannot see the queue. A
//! shape that owns one must observe this order, and no other:
//!
//! 1. Take everything available.
//! 2. [`Doorbell::clear`].
//! 3. **Check emptiness again.** If anything is there, do not wait -- go to 1.
//! 4. Wait on the handle.
//!
//! The re-check at step 3 is not an optimisation, and removing it is not a
//! missed wakeup once in a while -- it is a permanent hang. A producer that
//! pushes between steps 1 and 2 may signal before the clear at step 2 erases
//! it, leaving an item in the queue and the doorbell unsignalled. Nothing later
//! will signal again, because nothing later will arrive.
//!
//! Reversing steps 2 and 3 -- checking emptiness and then clearing -- fails the
//! same way and is the easier mistake to make, because it reads more naturally.
//! `spsc`'s test suite asserts this by reversing them deliberately and
//! requiring the result to hang.
//!
//! A lock-based queue gets this for free by clearing under the lock it already
//! holds while deciding there is nothing to take, which is what the file
//! watcher does. A lock-free queue has no such lock, so the ordering above is
//! the substitute, and it has to be written down because the compiler will not
//! ask about it.
//!
//! # Why a redundant signal is skipped, but a redundant clear is not
//!
//! The two directions are not symmetric, and the asymmetry is the reason this
//! type keeps a flag at all.
//!
//! A **late signal** is a spurious wakeup: a waiter wakes, finds nothing, and
//! waits again. A **stale clear** is a lost wakeup: a waiter sleeps on a
//! non-empty queue forever. Cheapening the signal side is therefore safe, and
//! cheapening the clear side is not.
//!
//! So `signal` keeps an [`AtomicBool`] mirroring the event and returns without
//! a syscall when the event is already signalled. On this crate's reference
//! machine `SetEvent` on an already-signalled event measured 81.2 ns against
//! 7.2 ns for an uncontended atomic, so a backlogged producer that would
//! otherwise pay a syscall per push pays roughly a tenth of one.
//!
//! The flag is allowed to disagree with the event briefly, and that is sound in
//! exactly one direction: it may claim signalled while the `SetEvent` has not
//! landed yet, which costs a skipped redundant signal, never a skipped
//! necessary one. It is never permitted to claim clear while the event is
//! signalled in a way that matters, because [`Doorbell::clear`] writes the flag
//! before touching the event.

use std::io;
use std::os::windows::io::{AsHandle, AsRawHandle, BorrowedHandle, FromRawHandle, OwnedHandle};
use std::ptr;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use windows_sys::Win32::Foundation::{DUPLICATE_SAME_ACCESS, DuplicateHandle, FALSE, TRUE};
use windows_sys::Win32::System::Threading::{
    CreateEventW, GetCurrentProcess, ResetEvent, SetEvent,
};

/// A lazily created manual-reset event that reports whether a queue has
/// anything to take.
///
/// See the [module documentation](self) for the arming protocol every owner
/// must follow; this type cannot enforce it, because it cannot see the queue.
pub(crate) struct Doorbell {
    /// The event, absent until somebody asks for the handle.
    event: OnceLock<OwnedHandle>,
    /// Mirrors the event's state so a redundant [`Doorbell::signal`] can skip
    /// its syscall. Only [`Doorbell::signal`] and [`Doorbell::clear`] write it.
    signalled: AtomicBool,
}

impl Doorbell {
    /// A doorbell that owns no kernel object yet.
    pub(crate) const fn new() -> Self {
        Self {
            event: OnceLock::new(),
            signalled: AtomicBool::new(false),
        }
    }

    /// Borrow the event, creating it on the first call.
    ///
    /// The borrow is deliberate: the event belongs to the queue and must not be
    /// closed by a caller. Use [`Doorbell::owned`] where ownership is required,
    /// such as arming a `ThreadpoolWait`.
    ///
    /// The event is created unsignalled regardless of what the queue holds,
    /// because this type cannot see the queue. The owner is responsible for
    /// bringing it into agreement, which the arming protocol does for free: the
    /// re-check after [`Doorbell::clear`] runs after creation, so an item that
    /// arrived before the doorbell existed is found rather than waited on.
    ///
    /// # Errors
    ///
    /// Returns the error from `CreateEventW` on the first call.
    pub(crate) fn handle(&self) -> io::Result<BorrowedHandle<'_>> {
        if let Some(event) = self.event.get() {
            return Ok(event.as_handle());
        }
        // A racing caller may win the `set`, in which case ours is dropped and
        // closed and theirs is used. Both are unsignalled, so the loser's
        // disappearance costs nothing; only one event can ever be published.
        let created = create_event()?;
        let _ = self.event.set(created);
        Ok(self
            .event
            .get()
            .expect("the doorbell was just published")
            .as_handle())
    }

    /// A duplicate of [`Doorbell::handle`] that the caller owns.
    ///
    /// The duplicate refers to the same event, so signalling reaches both, and
    /// the caller may close its copy whenever it likes. This is the form a
    /// `ThreadpoolWait` needs, since arming one takes ownership of its target.
    ///
    /// # Errors
    ///
    /// Returns the error from `CreateEventW` or `DuplicateHandle`.
    pub(crate) fn owned(&self) -> io::Result<OwnedHandle> {
        duplicate(self.handle()?)
    }

    /// Report that the queue has something to take.
    ///
    /// Does nothing when no handle has ever been requested, and nothing when
    /// the event is already signalled. Both skips are safe; see the [module
    /// documentation](self) for why the signal side may be cheapened and the
    /// clear side may not.
    ///
    /// A failure of `SetEvent` is not reported. There is no useful reaction on
    /// a producer's hot path, and the only documented failures are invalid
    /// handles, which cannot occur for an event this type owns for its whole
    /// lifetime.
    pub(crate) fn signal(&self) {
        let Some(event) = self.event.get() else {
            // Nobody is waiting on a handle that does not exist. A consumer
            // that creates one later re-checks the queue before waiting, so
            // this skip cannot strand an item.
            return;
        };
        if self.signalled.swap(true, Ordering::AcqRel) {
            // Already signalled, and a manual-reset event does not count, so
            // setting it again would change nothing.
            return;
        }
        // SAFETY: a live manual-reset event owned by this type for as long as
        // it exists; `SetEvent` has no other precondition.
        unsafe {
            SetEvent(event.as_raw_handle());
        }
    }

    /// Report that the queue appears to have nothing to take.
    ///
    /// **The caller must re-check emptiness after this returns**, and must not
    /// wait if the re-check finds anything. See the [module
    /// documentation](self); this is the step whose omission is a permanent
    /// hang rather than an occasional stall.
    pub(crate) fn clear(&self) {
        let Some(event) = self.event.get() else {
            return;
        };
        // Written before the event is reset, so a producer racing this call
        // sees a clear flag and issues a real `SetEvent`. That signal may then
        // be erased by the `ResetEvent` below -- which is precisely why the
        // caller's re-check, and not this ordering, is what carries the
        // guarantee.
        self.signalled.store(false, Ordering::Release);
        // SAFETY: as in `signal`.
        unsafe {
            ResetEvent(event.as_raw_handle());
        }
    }

    /// Whether the event has been created, for tests and for asserting that
    /// laziness actually holds.
    #[cfg(test)]
    pub(crate) fn is_armed(&self) -> bool {
        self.event.get().is_some()
    }
}

impl std::fmt::Debug for Doorbell {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Doorbell")
            .field("created", &self.event.get().is_some())
            .field("signalled", &self.signalled.load(Ordering::Relaxed))
            .finish()
    }
}

/// Create an unnamed, unsignalled, manual-reset event.
fn create_event() -> io::Result<OwnedHandle> {
    // SAFETY: creates an unnamed event with default security attributes; both
    // pointer arguments are null by design.
    let raw = unsafe { CreateEventW(ptr::null(), TRUE, FALSE, ptr::null()) };
    if raw.is_null() {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the call returned a fresh, exclusively owned event handle.
    Ok(unsafe { OwnedHandle::from_raw_handle(raw) })
}

/// Duplicate a handle into this process, so the caller owns its own copy.
fn duplicate(handle: BorrowedHandle<'_>) -> io::Result<OwnedHandle> {
    let mut duplicated = ptr::null_mut();
    // SAFETY: duplicates a live handle within this process with the same
    // access; `duplicated` is a valid out-pointer for the call's duration.
    let ok = unsafe {
        DuplicateHandle(
            GetCurrentProcess(),
            handle.as_raw_handle(),
            GetCurrentProcess(),
            &raw mut duplicated,
            0,
            FALSE,
            DUPLICATE_SAME_ACCESS,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the call succeeded, so `duplicated` is a fresh owned handle.
    Ok(unsafe { OwnedHandle::from_raw_handle(duplicated) })
}

#[cfg(test)]
mod tests;
