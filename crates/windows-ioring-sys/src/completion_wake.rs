// Copyright (c) 2026 Mike Grier
//! Wake-only Model A delivery (M33.1, [D-84](../DESIGN-NOTES.md#d-84)): the ring's completion event
//! wired to a thread-pool wait that tells the ring's owner to drain, for a caller that keeps its
//! ring.
//!
//! [`EventDelivery`](crate::EventDelivery) is built on this, so the protocol below is written once.

use std::io;

use windows_threadpool_sys::callback_env::CallbackEnviron;
use windows_threadpool_sys::wait::{ThreadpoolWait, WaitableHandle};

use crate::ring::IoRing;

/// Tells the owner of an [`IoRing`] to drain it, on a thread-pool thread, whenever the ring's
/// completion queue may have gone from empty to non-empty (Model A, D-3; D-84).
///
/// The counterpart to [`EventDelivery`](crate::EventDelivery) for a caller that keeps its ring --
/// behind its own lock, beside its own state -- and wants the thread pool only to say when to
/// drain. `EventDelivery` is this, with a drain that hands each completion to a callback, over a
/// ring it owns.
///
/// # What the owner owes
///
/// - **Drain to empty on every call.** The ring's event is an edge, raised when the completion
///   queue goes from empty to non-empty ([D-19](../DESIGN-NOTES.md#d-19)), so a call that leaves a
///   completion queued may never be followed by another.
/// - **Treat a call with nothing to pop as normal.** There is one at setup, and each activation
///   calls twice.
/// - **One waiter per ring** ([D-21](../DESIGN-NOTES.md#d-21)): no second wake over the same ring,
///   and no other wait on its event. Stated, not enforced, because the wake does not hold the ring.
/// - **Never drop the wake from inside its own call.** Dropping it waits for a running call, which
///   would be waiting for itself.
///
/// # What the owner may rely on
///
/// Both are contract, as they are for `EventDelivery` ([D-83](../DESIGN-NOTES.md#d-83)):
///
/// - **No lock and no ring is held while it calls.** The wake holds no reference to the ring, so
///   the owner may take its own lock in the call, pop, and push and submit more work.
/// - **Calls may overlap.** Two may run at once on different pool threads; the owner's own lock is
///   what serializes its ring.
pub struct CompletionWake {
    wait: ThreadpoolWait,
}

impl CompletionWake {
    /// Wire `ring`'s completion event to a thread-pool wait that calls `on_wake` on a pool thread
    /// whenever the ring's completion queue may have gone from empty to non-empty, and once at
    /// setup, so a backlog already queued is reached.
    ///
    /// The protocol is `EventDelivery`'s: the event is attached unsignalled, the wait armed, and
    /// only then the setup signal raised -- the order `SetThreadpoolWait` documents ("you must
    /// re-register the event with the wait object before signaling it each time to trigger the
    /// wait callback"); and each activation calls `on_wake`, re-arms, and calls `on_wake` again,
    /// the second call closing the gap a completion arriving before the re-arm would leave
    /// (`M4.2`, `M26.9`, `M26.12`).
    ///
    /// `env` binds the wait's pool -- to a NUMA node's, for instance; `None` is the process's
    /// default pool.
    ///
    /// # Errors
    ///
    /// [`io::ErrorKind::Unsupported`] if the running system does not report
    /// `IORING_FEATURE_SET_COMPLETION_EVENT`; this crate does not substitute a polling thread.
    /// Also any error from attaching the ring's completion event, from `ThreadpoolWait::new`, or
    /// from raising the setup signal.
    pub fn new<T, X, F>(
        ring: &mut IoRing<T, X>,
        on_wake: F,
        env: Option<&mut CallbackEnviron<'_>>,
    ) -> io::Result<Self>
    where
        F: Fn() + Send + Sync + 'static,
    {
        // The ring creates, owns, and attaches its own event and hands back a duplicate (D-20),
        // which leaves exactly one `SetIoRingCompletionEvent` call site in this crate. It is taken
        // *unsignalled*, and the setup signal raised only after `wait.arm` below.
        let event = ring.attach_completion_event_unsignalled()?;
        windows_threadpool_sys::trace_record!(
            "delivery",
            "event-attached",
            std::os::windows::io::AsRawHandle::as_raw_handle(&event) as usize
        );
        // SAFETY: `attach_completion_event_unsignalled` returns a duplicate of an auto-reset
        // event -- always a supported wait target, and never a mutex -- and that duplicate is
        // exclusively ours. The ring keeps its own separate handle, so nothing else closes this
        // one while a wait is pending on it, and the ring goes on signalling whichever copies
        // survive.
        let event = unsafe { WaitableHandle::assume_waitable(event) };
        let wait = ThreadpoolWait::new(
            event,
            move |activation| {
                // Call, re-arm, call again: the event auto-resets on wait and is set whenever a
                // completion lands, so the only gap this leaves is a completion arriving between
                // the owner's last pop and the re-arm -- and the second call closes exactly that
                // gap (M4.2).
                windows_threadpool_sys::trace_record!("delivery", "callback-entered");
                on_wake();
                activation.rearm(None);
                on_wake();
                windows_threadpool_sys::trace_record!("delivery", "callback-left");
            },
            env,
        )?;
        wait.arm(None);
        windows_threadpool_sys::trace_record!("delivery", "armed");

        // Only now, with the wait registered, is the setup signal raised -- the order
        // `SetThreadpoolWait` documents. This is what makes a backlog reachable, so a failure to
        // raise it is a failure to construct. Raised unconditionally, whoever attached the event
        // first (`M26.12`).
        ring.raise_setup_signal()?;

        Ok(Self { wait })
    }
}

impl Drop for CompletionWake {
    fn drop(&mut self) {
        // The wait is armed, so it owes a drain. Discharged here, before the wait is released:
        // `windows-threadpool-sys` reports an undischarged one and, under its `fail-fast`
        // feature, panics on it. On return no call is running and none will start.
        //
        // Leaving it to the field's own drop would change no behaviour -- `ThreadpoolWait`'s
        // `Drop` disarms and drains too -- only the report, which is why the sabotage that does
        // so is declared an equivalent mutant rather than a gap.
        self.wait.stop_and_drain();
    }
}
