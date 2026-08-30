// Copyright (c) 2026 Mike Grier
//! Model A delivery: the ring's completion event wired to a thread-pool wait
//! (M4).

use std::io;
use std::sync::{Arc, Mutex, MutexGuard};

use windows_sys::Win32::Storage::FileSystem::IORING_OP_CODE;
use windows_threadpool_sys::callback_env::CallbackEnviron;
use windows_threadpool_sys::wait::{ThreadpoolWait, WaitableHandle};

use crate::batch::Batch;
use crate::capability::RingVersion;
use crate::ring::{Completion, IoRing, Op, RingInfo};
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

    /// Lock the wrapped ring and return a scope for submitting work to it.
    ///
    /// A caller submits by opening a [`RingScope::batch`], the same ring the
    /// wait callback locks to pop completions. Mutex poisoning is absorbed
    /// here rather than surfaced: a panic elsewhere does not invalidate a ring
    /// handle, and the callback's own drain already takes the same view, so
    /// making every caller write `unwrap_or_else(PoisonError::into_inner)` only
    /// invited the inconsistency this method removes.
    ///
    /// # What the scope deliberately withholds
    ///
    /// The rule is: **every read-only part of [`IoRing`], plus batch
    /// construction -- and nothing that can retarget the ring or steal the
    /// pool's completions.** So there is no `try_pop` (D-21 makes the pool the
    /// single drainer; a second one breaks the drain-to-empty that re-arms the
    /// edge), no `completion_event` (it hands back a duplicate of the event
    /// this `EventDelivery` already waits on, giving two waiters on one ring),
    /// no `run_down`, and above all **no `&mut IoRing`**.
    ///
    /// That last one is the point, and it is why this returns a scope rather
    /// than the `&Mutex<IoRing>` it used to. Any `&mut IoRing` permits
    /// whole-value assignment, so safe code could replace the ring while the
    /// pool's wait stayed armed on the *original* ring's event -- measured, and
    /// delivery stopped silently ([D-43](../DESIGN-NOTES.md#d-43)). Note a
    /// `Deref`/`DerefMut` newtype would not have closed that, since
    /// `*scope = ...` works through `DerefMut` just as well; nor would handing
    /// a `&mut IoRing` to a closure.
    ///
    /// Replacing the ring is therefore refused at compile time:
    ///
    /// ```compile_fail
    /// # use windows_ioring_sys::{EventDelivery, IoRing};
    /// let delivery =
    ///     EventDelivery::new(IoRing::new(8, 8).unwrap(), |_| {}, None).unwrap();
    /// let mut scope = delivery.scope();
    /// // No `DerefMut`, so there is no `&mut IoRing` to assign through.
    /// *scope = IoRing::new(8, 8).unwrap();
    /// ```
    ///
    /// The same setup doing the legitimate thing must still compile. A
    /// `compile_fail` example passes on *any* error, including a typo in its
    /// own setup, so this pair is what keeps the one above honest:
    ///
    /// ```
    /// # use windows_ioring_sys::{EventDelivery, IoRing};
    /// let delivery =
    ///     EventDelivery::new(IoRing::new(8, 8).unwrap(), |_| {}, None).unwrap();
    /// let scope = delivery.scope();
    /// assert_eq!(scope.outstanding(), 0);
    /// ```
    #[must_use]
    pub fn scope(&self) -> RingScope<'_> {
        RingScope {
            ring: self
                .ring
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        }
    }
}

/// Exclusive access to an [`EventDelivery`]'s ring, narrowed to what a
/// submitting caller needs (M18.6).
///
/// Held for as long as the value lives, so keep it to the shortest scope that
/// covers a submission: the wait callback needs the same lock to deliver
/// completions.
///
/// See [`EventDelivery::scope`] for what this deliberately does not expose,
/// and why handing out anything that yields a `&mut IoRing` would reopen
/// [D-43](../DESIGN-NOTES.md#d-43).
pub struct RingScope<'delivery> {
    ring: MutexGuard<'delivery, IoRing>,
}

impl RingScope<'_> {
    /// Open a [`Batch`] against the ring.
    ///
    /// The borrow is confined to the returned batch, so no `&mut IoRing`
    /// escapes to the caller.
    pub fn batch(&mut self) -> Batch<'_> {
        Batch::new(&mut self.ring)
    }

    /// Operations submitted but not yet popped, as [`IoRing::outstanding`].
    #[must_use]
    pub fn outstanding(&self) -> usize {
        self.ring.outstanding()
    }

    /// This ring's negotiated version, as [`IoRing::version`].
    #[must_use]
    pub fn version(&self) -> RingVersion {
        self.ring.version()
    }

    /// Query the ring, as [`IoRing::info`].
    ///
    /// # Errors
    ///
    /// As [`IoRing::info`].
    pub fn info(&self) -> io::Result<RingInfo> {
        self.ring.info()
    }

    /// Whether this ring supports `op`, as [`IoRing::supports`].
    #[must_use]
    pub fn supports(&self, op: Op) -> bool {
        self.ring.supports(op)
    }

    /// Whether this ring supports `op_code`, as [`IoRing::supports_raw`].
    #[must_use]
    pub fn supports_raw(&self, op_code: IORING_OP_CODE) -> bool {
        self.ring.supports_raw(op_code)
    }

    /// Registered file count, as [`IoRing::registered_file_count`].
    #[must_use]
    pub fn registered_file_count(&self) -> u32 {
        self.ring.registered_file_count()
    }

    /// Registered buffer count, as [`IoRing::registered_buffer_count`].
    #[must_use]
    pub fn registered_buffer_count(&self) -> u32 {
        self.ring.registered_buffer_count()
    }
}

#[cfg(test)]
mod tests;
