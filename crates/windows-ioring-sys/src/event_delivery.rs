// Copyright (c) 2026 Mike Grier
//! Model A delivery: `SetIoRingCompletionEvent` wired to a thread-pool wait
//! (M4).

use std::io;
use std::os::windows::io::AsRawHandle;
use std::sync::{Arc, Mutex};

use windows_sys::Win32::Storage::FileSystem::SetIoRingCompletionEvent;
use windows_threadpool_sys::callback_env::CallbackEnviron;
use windows_threadpool_sys::wait::{ThreadpoolWait, WaitableHandle};

use crate::capability::capabilities;
use crate::error::check;
use crate::ring::{Completion, IoRing};

/// Pop every completion currently available and hand each to `on_completion`.
fn drain(ring: &Mutex<IoRing>, on_completion: &(dyn Fn(Completion) + Send + Sync)) {
    let mut ring = ring
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    loop {
        match ring.try_pop() {
            Ok(Some(completion)) => on_completion(completion),
            Ok(None) => break,
            Err(error) => {
                debug_assert!(
                    false,
                    "IoRing::try_pop failed during event-driven drain: {error}"
                );
                break;
            }
        }
    }
}

/// Delivers an [`IoRing`]'s completions on thread-pool callback threads via
/// its completion event, rather than a caller polling [`IoRing::try_pop`]
/// itself (Model A, D-3).
///
/// # Not a `CleanupGroup` member
///
/// There is no way to add an already-built `EventDelivery` to a
/// [`CleanupGroup`](windows_threadpool_sys::cleanup_group::CleanupGroup), for
/// the same reason `CleanupGroup` excludes
/// [`ThreadpoolIo`](windows_threadpool_sys::io::ThreadpoolIo): the ring must
/// run down its outstanding operations before closing, and a group's bulk
/// `CloseThreadpoolCleanupGroupMembers` has no way to run that logic for a
/// member it did not create itself. `EventDelivery` stays individually
/// owned, where its own field-drop order (below) gives the same
/// quiesce-then-close guarantee a group would otherwise provide.
pub struct EventDelivery {
    // Drop order matters and is why these fields are declared in this order:
    // Rust drops struct fields top-to-bottom. `wait` must go first -- its own
    // `Drop` disarms, suppresses re-arming, and drains any in-flight callback
    // before releasing its captured `Arc<Mutex<IoRing>>` clone -- so that by
    // the time `ring`'s last reference drops below and runs
    // `IoRing::run_down` then `CloseIoRing`, no callback can still be
    // touching it (M4.3).
    #[allow(
        dead_code,
        reason = "held only for its Drop side effect and ordering relative to `ring`"
    )]
    wait: ThreadpoolWait,
    ring: Arc<Mutex<IoRing>>,
}

impl EventDelivery {
    /// Wire `ring`'s completion event to a thread-pool wait, delivering every
    /// popped [`Completion`] to `on_completion` on a pool thread (M4.2).
    ///
    /// The wait is armed before this returns: the calling thread never waits
    /// for a completion itself (M4.4), including any that were already
    /// queued when `ring` was handed over.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::Unsupported`] if the running system does not
    /// report `IORING_FEATURE_SET_COMPLETION_EVENT` (M4.1). This crate
    /// refuses to silently substitute a thread-based polling loop instead --
    /// a caller who asked for event-driven delivery and got a spun-up thread
    /// has been told something false. Also returns any error from
    /// `SetIoRingCompletionEvent` or `ThreadpoolWait::new`.
    pub fn new<F>(
        ring: IoRing,
        on_completion: F,
        env: Option<&mut CallbackEnviron<'_>>,
    ) -> io::Result<Self>
    where
        F: Fn(Completion) + Send + Sync + 'static,
    {
        if !capabilities()?.supports_completion_event {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "this system's IoRing does not report IORING_FEATURE_SET_COMPLETION_EVENT",
            ));
        }

        let event = WaitableHandle::event(false, false)?;
        // SAFETY: `ring`'s handle is live; the event handle stays open at
        // least until `ThreadpoolWait::new` takes ownership of it below.
        let hr =
            unsafe { SetIoRingCompletionEvent(ring.raw_handle(), event.handle().as_raw_handle()) };
        check(hr)?;

        let ring = Arc::new(Mutex::new(ring));
        let ring_for_wait = Arc::clone(&ring);
        let on_completion: Arc<dyn Fn(Completion) + Send + Sync> = Arc::new(on_completion);
        let wait = ThreadpoolWait::new(
            event,
            move |activation| {
                // Drain-to-empty, re-arm, drain-to-empty again: the event
                // auto-resets on wait and is set whenever a completion
                // lands, so the only gap this leaves is a completion that
                // arrives between the last pop and the re-arm -- and the
                // second drain closes exactly that gap (M4.2).
                drain(&ring_for_wait, on_completion.as_ref());
                activation.rearm(None);
                drain(&ring_for_wait, on_completion.as_ref());
            },
            env,
        )?;
        wait.arm(None);

        Ok(Self { wait, ring })
    }

    /// The wrapped ring, shared with the wait callback above.
    ///
    /// A caller submits new work by locking this and building a
    /// [`crate::Batch`] against it, the same way the callback locks it to
    /// pop completions.
    #[must_use]
    pub fn ring(&self) -> &Mutex<IoRing> {
        &self.ring
    }
}

#[cfg(test)]
mod tests;
