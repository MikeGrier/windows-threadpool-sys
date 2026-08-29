// Copyright (c) 2026 Mike Grier
//! Model A delivery: the ring's completion event wired to a thread-pool wait
//! (M4).

use std::io;
use std::sync::{Arc, Mutex};

use windows_threadpool_sys::callback_env::CallbackEnviron;
use windows_threadpool_sys::wait::{ThreadpoolWait, WaitableHandle};

use crate::ring::{Completion, IoRing};

/// Pop every completion currently available and hand each to `on_completion`.
///
/// Each pop is its own short lock: `on_completion` always runs with the
/// mutex released, so a slow callback does not block a submitter, and a
/// callback that calls [`EventDelivery::ring`] and locks it itself cannot
/// deadlock against this loop.
fn drain(ring: &Mutex<IoRing>, on_completion: &(dyn Fn(Completion) + Send + Sync)) {
    loop {
        let popped = {
            let mut ring = ring
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            ring.try_pop()
        };
        match popped {
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
    /// The wait is armed before this returns, so the calling thread never
    /// waits for a completion itself (M4.4).
    ///
    /// # Completions already queued when `ring` is handed over
    ///
    /// They are delivered too -- but that is a guarantee this method has to
    /// buy, not one it inherits, and saying so is the point of this section.
    /// The ring's event is edge-triggered on the completion queue going empty
    /// to non-empty ([`IoRing::completion_event`], D-19), so attaching to a
    /// ring whose queue is *already* non-empty signals nothing, and no later
    /// completion signals either, because the queue never returns to empty to
    /// re-arm the edge. Such a backlog is stranded permanently.
    ///
    /// What closes that gap is the deliberate signal
    /// [`IoRing::completion_event`] raises as it attaches: the first callback
    /// then drains the backlog exactly as it would any other wakeup.
    ///
    /// This was false in the implementation, and asserted anyway in this
    /// rustdoc, before M11.3 -- every test until then handed over a fresh
    /// ring, so nothing contradicted it. A caller on an earlier version
    /// cannot rely on the guarantee; `tests/event_delivery.rs` keeps the
    /// repro that now holds it.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::Unsupported`] if the running system does not
    /// report `IORING_FEATURE_SET_COMPLETION_EVENT` (M4.1), which
    /// [`IoRing::completion_event`] is what decides. This crate refuses to
    /// silently substitute a thread-based polling loop instead -- a caller
    /// who asked for event-driven delivery and got a spun-up thread has been
    /// told something false. Also returns any other error from
    /// [`IoRing::completion_event`] or from `ThreadpoolWait::new`.
    pub fn new<F>(
        mut ring: IoRing,
        on_completion: F,
        env: Option<&mut CallbackEnviron<'_>>,
    ) -> io::Result<Self>
    where
        F: Fn(Completion) + Send + Sync + 'static,
    {
        // The ring creates, owns, and attaches its own event and hands back a
        // duplicate (D-20), which leaves exactly one
        // `SetIoRingCompletionEvent` call site in this crate. Delegating also
        // means the capability check, the `Unsupported` error, and the
        // signal-once-on-attach that makes the backlog guarantee above true
        // are each stated in one place rather than restated here.
        let event = ring.completion_event()?;
        // SAFETY: `completion_event` returns a duplicate of an auto-reset
        // event -- always a supported wait target, and never a mutex -- and
        // that duplicate is exclusively ours. The ring keeps its own separate
        // handle, so nothing else closes this one while a wait is pending on
        // it, and the ring goes on signalling whichever copies survive.
        let event = unsafe { WaitableHandle::assume_waitable(event) };

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
    ///
    /// **Do not call [`IoRing::completion_event`] on it.** That call
    /// succeeds -- it returns a duplicate of the very event this
    /// `EventDelivery` is already waiting on, since the ring attaches at most
    /// one (D-20) -- and the result is two waiters on one ring's event, which
    /// D-21 says cannot be made correct: the drain that restores the empty
    /// state, and so re-arms the edge, has to run to empty exactly once.
    /// Before M11.3 the same call was worse rather than better, silently
    /// detaching the pool's event by replacing it, so delivery simply
    /// stopped. A consumer that wants to wait on the ring itself wants Model
    /// B with a multiplexed wakeup instead, and should not hand its ring to
    /// `EventDelivery` at all.
    #[must_use]
    pub fn ring(&self) -> &Mutex<IoRing> {
        &self.ring
    }
}

#[cfg(test)]
mod tests;
