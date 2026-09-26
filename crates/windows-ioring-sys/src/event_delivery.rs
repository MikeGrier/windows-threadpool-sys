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
/// What a delivery hands a caller for each completion.
///
/// The completion itself, plus whatever the ring was holding for it -- `None`
/// when the push carried nothing to give back (`M28.5`), or when this ring was
/// never holding anything for that identity.
type OnCompletion<T, X> = dyn Fn(Completion, Option<(Option<T>, X)>) + Send + Sync;

fn drain<T, X>(ring: &Mutex<IoRing<T, X>>, on_completion: &OnCompletion<T, X>) {
    loop {
        let popped = {
            let mut ring = ring
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            ring.try_pop()
        };
        match popped {
            Ok(Some((completion, held))) => on_completion(completion, held),
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
pub struct EventDelivery<T = (), X = ()> {
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
    ring: Arc<Mutex<IoRing<T, X>>>,
}

impl<T: Send + 'static, X: Send + 'static> EventDelivery<T, X> {
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
    /// What closes that gap is a deliberate signal raised on the event once
    /// it has been attached: the first callback then drains the backlog
    /// exactly as it would any other wakeup.
    ///
    /// That signal is raised *after* the wait has been armed, which is the
    /// order `SetThreadpoolWait` documents -- "you must re-register the event
    /// with the wait object before signaling it each time to trigger the wait
    /// callback". Signalling first and arming afterwards is not guaranteed to
    /// run the callback, and since the event is auto-reset the signal is
    /// consumed rather than left pending for the arming to find. For a ring
    /// whose queue never returns to empty there is no second wakeup coming,
    /// so that loss strands the backlog permanently instead of merely
    /// delaying it. This method therefore attaches the event unsignalled and
    /// raises the signal itself, rather than going through
    /// [`IoRing::completion_event`], which signals as it attaches and so
    /// leaves a caller no way to arm in between.
    ///
    /// This was false in the implementation, and asserted anyway in this
    /// rustdoc, before M11.3 -- every test until then handed over a fresh
    /// ring, so nothing contradicted it. A caller on an earlier version
    /// cannot rely on the guarantee; `tests/event_delivery.rs` keeps the
    /// repro that now holds it. The ordering above was wrong until M26.9, in
    /// a way that stranded the backlog in roughly one run in a hundred.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::Unsupported`] if the running system does not
    /// report `IORING_FEATURE_SET_COMPLETION_EVENT` (M4.1), which
    /// [`IoRing::completion_event`] is what decides. This crate refuses to
    /// silently substitute a thread-based polling loop instead -- a caller
    /// who asked for event-driven delivery and got a spun-up thread has been
    /// told something false. Also returns any other error from attaching the
    /// ring's completion event, from `ThreadpoolWait::new`, or from raising
    /// the setup signal.
    pub fn new<F>(
        mut ring: IoRing<T, X>,
        on_completion: F,
        env: Option<&mut CallbackEnviron<'_>>,
    ) -> io::Result<Self>
    where
        F: Fn(Completion, Option<(Option<T>, X)>) + Send + Sync + 'static,
    {
        // The ring creates, owns, and attaches its own event and hands back a
        // duplicate (D-20), which leaves exactly one
        // `SetIoRingCompletionEvent` call site in this crate. Delegating also
        // means the capability check and the `Unsupported` error are each
        // stated in one place rather than restated here.
        //
        // The attachment is taken *unsignalled*, and the setup signal raised
        // only after `wait.arm` below, because `SetThreadpoolWait` documents
        // that "you must re-register the event with the wait object before
        // signaling it each time to trigger the wait callback". Signalling
        // first and arming afterwards is the order that rule forbids.
        let (event, owes_setup_signal) = ring.attach_completion_event_unsignalled()?;
        windows_threadpool_sys::trace_record!(
            "delivery",
            "event-attached",
            std::os::windows::io::AsRawHandle::as_raw_handle(&event) as usize
        );
        // SAFETY: `completion_event` returns a duplicate of an auto-reset
        // event -- always a supported wait target, and never a mutex -- and
        // that duplicate is exclusively ours. The ring keeps its own separate
        // handle, so nothing else closes this one while a wait is pending on
        // it, and the ring goes on signalling whichever copies survive.
        let event = unsafe { WaitableHandle::assume_waitable(event) };

        let ring = Arc::new(Mutex::new(ring));
        let ring_for_wait = Arc::clone(&ring);
        let on_completion: Arc<OnCompletion<T, X>> = Arc::new(on_completion);
        let wait = ThreadpoolWait::new(
            event,
            move |activation| {
                // Drain-to-empty, re-arm, drain-to-empty again: the event
                // auto-resets on wait and is set whenever a completion
                // lands, so the only gap this leaves is a completion that
                // arrives between the last pop and the re-arm -- and the
                // second drain closes exactly that gap (M4.2).
                windows_threadpool_sys::trace_record!("delivery", "callback-entered");
                drain(&ring_for_wait, on_completion.as_ref());
                activation.rearm(None);
                drain(&ring_for_wait, on_completion.as_ref());
                windows_threadpool_sys::trace_record!("delivery", "callback-left");
            },
            env,
        )?;
        wait.arm(None);
        windows_threadpool_sys::trace_record!("delivery", "armed");

        // Only now, with the wait registered, is the setup signal raised --
        // the order `SetThreadpoolWait` documents. This is what makes the
        // backlog guarantee above true, so a failure to raise it is a failure
        // to construct.
        //
        // Signalled only when this call attached the event. A caller that
        // attached it earlier and consumed the signal reaches this with a
        // non-empty queue and no wakeup owing, which review raised and
        // `M26.12` is investigating -- the obvious repair, signalling
        // unconditionally, was tried and does **not** fix it, so it is not
        // applied here. See the ignored reproducer in
        // `tests/event_delivery.rs` and UNRESOLVED-TEST-FAILURES.md.
        if owes_setup_signal {
            ring.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .raise_setup_signal()?;
        }

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
    ///     EventDelivery::new(IoRing::new(8, 8).unwrap(), |_, _| {}, None).unwrap();
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
    ///     EventDelivery::new(IoRing::new(8, 8).unwrap(), |_, _| {}, None).unwrap();
    /// let scope = delivery.scope();
    /// assert_eq!(scope.outstanding(), 0);
    /// ```
    #[must_use]
    pub fn scope(&self) -> RingScope<'_, T, X> {
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
pub struct RingScope<'delivery, T = (), X = ()> {
    ring: MutexGuard<'delivery, IoRing<T, X>>,
}

impl<T, X> RingScope<'_, T, X> {
    /// Open a [`Batch`] against the ring.
    ///
    /// The borrow is confined to the returned batch, so no `&mut IoRing`
    /// escapes to the caller.
    pub fn batch(&mut self) -> Batch<'_, T, X> {
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
