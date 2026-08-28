// Copyright (c) Mike Grier.

//! Whether `CancelSynchronousIo` is safe to point at a shared thread.
//!
//! A mid-flight cancellation scheme wanted to cancel a wedged namespace
//! operation on a pool worker. The scheme guarded the cancel with a lock the
//! worker also takes, which closes the *user-mode* window -- but the whole
//! argument rested on an unverified kernel property: that
//! `CancelSynchronousIo` walks the target thread's IRP list **before it
//! returns**, so it either finds the operation or finds nothing.
//!
//! If instead a cancel could linger as a request against the *thread*, it would
//! land on whatever that thread did next -- and on a shared pool worker, that is
//! another crate's I/O. No user-mode lock can close that.
//!
//! # What was measured, and why it ends the discussion
//!
//! The sticky-versus-point-in-time question turned out to be the *lesser*
//! finding. The original hammer loop wedged with the **canceller** blocked
//! inside `ntdll!NtCancelSynchronousIoFile` while the target thread sat in
//! `ntdll!NtReadFile` -- zero CPU, four identical noninvasive samples over
//! twelve seconds. So `CancelSynchronousIo` **can block indefinitely**.
//!
//! That matters far more than stickiness, because in the proposed design the
//! canceller is a control-plane thread and the target is a shared worker: a
//! control plane that can be wedged by the thing it is trying to rescue is not
//! a control plane. This is the measurement behind mid-flight cancellation
//! staying deferred.
//!
//! Migrated from the throwaway `ctx-probe` spike (Probes G and H).
//!
//! # Tier: binary only
//!
//! **Not a test, and must not become one.** The behaviour being measured is a
//! hang: the probe's own subject is a call that does not return. Every case
//! therefore runs on its own thread behind a watchdog and reports a wedge
//! rather than waiting for it, which is exactly the structure a test harness
//! cannot provide for itself -- a wedged `#[test]` takes the whole suite with
//! it.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{
    CloseHandle, DUPLICATE_SAME_ACCESS, DuplicateHandle, GetLastError, HANDLE,
};
use windows_sys::Win32::System::IO::CancelSynchronousIo;
use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetCurrentThread};

/// How long a case may run before it is declared wedged.
///
/// Generous by design: the finding is that a call blocks *indefinitely*, so the
/// only question is whether it returns at all.
pub const WATCHDOG: Duration = Duration::from_secs(5);

/// What one cancellation attempt did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CancelOutcome {
    /// The call returned, reporting that it found an operation to cancel.
    Cancelled,
    /// The call returned, reporting that it found nothing.
    ///
    /// `ERROR_NOT_FOUND` is the documented answer for a thread with no
    /// synchronous I/O outstanding, and is what a point-in-time cancel gives.
    NotFound {
        /// The raw code, so a different failure is not read as this one.
        code: u32,
    },
    /// The call did not return within [`WATCHDOG`].
    ///
    /// **This is the finding.** A canceller that can wedge cannot be a control
    /// plane for the thread it is trying to rescue.
    Wedged,
}

impl CancelOutcome {
    /// The call returned at all, whatever it reported.
    #[must_use]
    pub fn returned(self) -> bool {
        !matches!(self, Self::Wedged)
    }
}

/// A duplicated handle to the calling thread, usable from another thread.
///
/// `GetCurrentThread` returns a pseudo-handle that means "whoever is asking",
/// so it must be duplicated into a real handle before a canceller can name this
/// thread with it. Getting that wrong would have the canceller cancel itself.
struct ThreadHandle(HANDLE);

// SAFETY: a real thread handle is process-wide rather than thread-affine, and
// is only ever passed to CancelSynchronousIo, which Windows serialises.
unsafe impl Send for ThreadHandle {}
// SAFETY: as above.
unsafe impl Sync for ThreadHandle {}

impl ThreadHandle {
    /// Duplicates the calling thread's pseudo-handle into a real one.
    ///
    /// # Panics
    ///
    /// Panics if the duplication fails, since the probe cannot proceed without
    /// naming its target.
    fn for_current_thread() -> Self {
        let mut duplicate: HANDLE = std::ptr::null_mut();
        // SAFETY: both process arguments are the current-process pseudo-handle,
        // which is what DuplicateHandle wants for a same-process duplication;
        // GetCurrentThread is the source; `duplicate` is writable.
        let duplicated = unsafe {
            let process = GetCurrentProcess();
            DuplicateHandle(
                process,
                GetCurrentThread(),
                process,
                &raw mut duplicate,
                0,
                0,
                DUPLICATE_SAME_ACCESS,
            )
        };
        assert_ne!(duplicated, 0, "duplicate the current thread's handle");

        Self(duplicate)
    }
}

impl Drop for ThreadHandle {
    fn drop(&mut self) {
        // SAFETY: the handle is owned here and closed exactly once.
        unsafe { CloseHandle(self.0) };
    }
}

/// Cancels synchronous I/O on `target`, under a watchdog.
///
/// The watchdog is the entire reason this is a library function rather than an
/// inline call: the measured behaviour is that this can block forever, so a
/// caller that simply called it would be the thing that hangs.
fn cancel_under_watchdog(target: Arc<ThreadHandle>) -> CancelOutcome {
    let finished = Arc::new(AtomicBool::new(false));
    let result = Arc::new(std::sync::Mutex::new(CancelOutcome::Wedged));

    {
        let finished = Arc::clone(&finished);
        let result = Arc::clone(&result);

        std::thread::spawn(move || {
            // SAFETY: `target` is a real thread handle, live for this call.
            let cancelled = unsafe { CancelSynchronousIo(target.0) };
            let outcome = if cancelled != 0 {
                CancelOutcome::Cancelled
            } else {
                // SAFETY: no preconditions.
                CancelOutcome::NotFound {
                    code: unsafe { GetLastError() },
                }
            };

            *result.lock().expect("the result is not poisoned") = outcome;
            finished.store(true, Ordering::SeqCst);
        });
    }

    let deadline = Instant::now() + WATCHDOG;
    while Instant::now() < deadline && !finished.load(Ordering::SeqCst) {
        std::thread::sleep(Duration::from_millis(10));
    }

    // A wedged canceller thread is deliberately left running: it cannot be
    // killed safely, and the process is about to exit. That this is the only
    // option is itself part of the finding.
    *result.lock().expect("the result is not poisoned")
}

/// Cancels against a thread that provably has **no** I/O outstanding.
///
/// The decisive case for the sticky-versus-point-in-time question. If a cancel
/// issued here landed on the target's *next* operation, the cancel would be a
/// standing request against the thread -- and on a shared pool worker that is
/// another crate's I/O, which no user-mode lock can protect.
#[must_use]
pub fn cancel_against_idle_thread() -> CancelOutcome {
    let target = Arc::new(ThreadHandle::for_current_thread());

    // This thread is doing nothing but waiting, so it has no synchronous I/O
    // outstanding by construction.
    cancel_under_watchdog(target)
}

/// Cancels repeatedly against a thread that is re-entering synchronous I/O.
///
/// The working hypothesis for the wedge: the cancel waits for the target to
/// reach a quiescent point, so it blocks when that thread immediately re-enters
/// synchronous I/O instead of returning to a wait.
///
/// Returns each attempt's outcome. A [`CancelOutcome::Wedged`] anywhere in the
/// result is the finding.
#[must_use]
pub fn cancel_against_busy_thread(attempts: usize) -> Vec<CancelOutcome> {
    let stop = Arc::new(AtomicBool::new(false));
    let handle = Arc::new(std::sync::Mutex::new(None::<Arc<ThreadHandle>>));

    let worker = {
        let stop = Arc::clone(&stop);
        let handle = Arc::clone(&handle);

        std::thread::spawn(move || {
            *handle.lock().expect("not poisoned") =
                Some(Arc::new(ThreadHandle::for_current_thread()));

            // Hammer synchronous I/O so the thread is rarely quiescent. Reading
            // a real file keeps the I/O genuinely synchronous rather than
            // satisfied from a user-mode buffer.
            let path = std::env::temp_dir().join(format!(
                "windows-platform-probes-cancel-{}.tmp",
                std::process::id()
            ));
            let _ = std::fs::write(&path, vec![0_u8; 1 << 20]);

            while !stop.load(Ordering::SeqCst) {
                let _ = std::fs::read(&path);
            }

            let _ = std::fs::remove_file(&path);
        })
    };

    // Wait for the worker to publish its handle.
    let deadline = Instant::now() + Duration::from_secs(2);
    let target = loop {
        if let Some(target) = handle.lock().expect("not poisoned").clone() {
            break target;
        }
        assert!(Instant::now() < deadline, "the worker never started");
        std::thread::sleep(Duration::from_millis(5));
    };

    let outcomes: Vec<CancelOutcome> = (0..attempts)
        .map(|_| cancel_under_watchdog(Arc::clone(&target)))
        .collect();

    stop.store(true, Ordering::SeqCst);
    let _ = worker.join();

    outcomes
}
