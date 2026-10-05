// Copyright (c) 2026 Mike Grier
// Split from ring.rs at 265a7a04.
//! What the ring knows about itself: negotiated capabilities, the completion
//! event, registration counts, the operation ledger, and rundown.
//!
//! A child of [`super`] rather than a sibling, because Rust's privacy is
//! asymmetric: rundown here calls the parent's private `is_quiescent` and
//! `drain_for_rundown`, which must stay up there so `Drop` can reach them too.

use super::*;

impl<T, X> IoRing<T, X> {
    /// The version this ring was created at.
    #[must_use]
    pub fn version(&self) -> RingVersion {
        self.version
    }

    /// Query this ring's current info via `GetIoRingInfo`.
    ///
    /// # Errors
    ///
    /// Returns any error from `GetIoRingInfo`.
    pub fn info(&self) -> io::Result<RingInfo> {
        let mut raw = IORING_INFO::default();
        // SAFETY: `self.handle` is a live ring; `raw` is a valid out-pointer.
        let hr = unsafe { GetIoRingInfo(self.handle, &raw mut raw) };
        check(hr)?;
        Ok(RingInfo {
            version: RingVersion::from_raw(raw.IoRingVersion),
            submission_queue_size: raw.SubmissionQueueSize,
            completion_queue_size: raw.CompletionQueueSize,
        })
    }

    /// Whether this ring supports `op`, from the capability set cached at
    /// construction.
    ///
    /// Answers what the *kernel's* op table contains, not what this crate's
    /// safe push surface reaches (M10.1, [`Op`]'s own docs list the mapping).
    /// The two coincide for every op except [`Op::Nop`], which has no
    /// [`crate::Batch`] method at all: a nop owns no buffer, so there is
    /// nothing for the ring to hand back, and it is reachable only
    /// through [`IoRing::push_raw`]. A `true` here therefore means "the
    /// kernel would accept this op", not "a `Batch` method exists to push
    /// it".
    #[must_use]
    pub fn supports(&self, op: Op) -> bool {
        self.supported_ops.contains(op)
    }

    /// Overrides the cached capability set to exactly `ops`, for tests that
    /// need a ring known to lack support for something.
    ///
    /// Every real host this crate has been tested against supports all seven
    /// named ops, which is exactly why [`IoRing::supports`] and
    /// [`Batch::require`](crate::Batch)'s use of it could not be told apart
    /// from a constant `true` by any test that only ever asked a real ring:
    /// the honest answer and the constant agree on every host available to
    /// run the test. This seam constructs the disagreement instead of hoping
    /// to find a host that has it.
    ///
    /// Not available outside `#[cfg(test)]`, for the same reason
    /// [`Completion::synthetic`] is not: production code has no legitimate
    /// reason to claim a capability the kernel did not actually report.
    #[cfg(test)]
    pub(crate) fn set_supported_ops_for_test(&mut self, ops: &[Op]) {
        self.supported_ops = OpSupport(ops.iter().fold(0_u8, |mask, &op| {
            let index = Op::ALL
                .iter()
                .position(|&candidate| candidate == op)
                .expect("Op::ALL is exhaustive");
            mask | (1 << index)
        }));
    }

    /// An owned duplicate of this ring's completion event, so a caller can
    /// wait on the ring alongside other handles without surrendering it
    /// (M11.1, D-20).
    ///
    /// The ring creates the event, attaches it with
    /// `SetIoRingCompletionEvent`, keeps ownership, and hands back a
    /// duplicate. Closing the returned handle is therefore always safe: the
    /// ring keeps signalling its own copy, and a `DuplicateHandle`'d event is
    /// still signalled after the original is closed.
    ///
    /// **Idempotent.** Repeat calls return another duplicate of the *same*
    /// event rather than attaching a new one, so two subsystems can each ask
    /// without silently detaching the other's.
    ///
    /// This is not a third delivery architecture -- it is Model B with a
    /// multiplexed wakeup source, changing only what the domain thread blocks
    /// on, never who owns, submits, or drains ([`Batch::submit_and_wait`] is
    /// still the single-source shape). Use it when ring I/O has to be waited
    /// on alongside non-ring I/O, which
    /// `IOSQE_FLAGS_DRAIN_PRECEDING_OPS` cannot order across.
    ///
    /// [`Batch::submit_and_wait`]: crate::Batch::submit_and_wait
    ///
    /// # The event is an edge, not a level
    ///
    /// **Measured, not inferred, and getting it wrong hangs rather than just
    /// slowing down** (D-19). The event is signalled when the completion
    /// queue transitions from **empty to non-empty**. It is *not* signalled
    /// once per completion, and it is *not* level-triggered: eight
    /// completions arriving at once into an empty queue produce exactly one
    /// wakeup, and a completion arriving into an already-non-empty queue
    /// produces none.
    ///
    /// Two rules follow, and they are contract rather than advice:
    ///
    /// 1. **Drain to empty before waiting again** -- [`IoRing::try_pop`]
    ///    until it yields `None`, on *every* pass through a multiplexed wait
    ///    loop, not only the pass where this handle signalled. A wait entered
    ///    with entries still in the queue blocks until some later completion
    ///    arrives *after* the queue has been emptied, which may be never.
    ///    That is a lost-wakeup deadlock, not a latency wobble.
    /// 2. **A wake with nothing to pop is normal.** It must not be treated as
    ///    an error or as a spurious wakeup. This method deliberately produces
    ///    one: the event is signalled once before it returns, so a caller who
    ///    had already submitted work never misses a backlog that arrived
    ///    before the event was attached.
    ///
    /// The event is auto-reset, and **exactly one waiter per ring** is
    /// supported (D-21): the drain that restores the empty state, and so
    /// re-arms the edge, must run to empty exactly once. Two threads waiting
    /// on one ring's event cannot be made correct.
    ///
    /// # If you are arming a thread-pool wait on this handle
    ///
    /// `SetThreadpoolWait` documents that "you must re-register the event
    /// with the wait object before signaling it each time to trigger the wait
    /// callback". This method signals the event *before* it returns (rule 2
    /// above), so by the time you have a handle to build a wait object
    /// around, that setup signal has already happened -- in the order the
    /// rule forbids, and so with no documented guarantee that your callback
    /// runs. Combined with the edge rule above, a ring whose queue never
    /// returns to empty has no second wakeup coming, so a setup signal that
    /// does not reach your callback strands the backlog permanently rather
    /// than merely delaying it.
    ///
    /// The remedy needs nothing this method does not already give you --
    /// after arming the wait, signal your own duplicate yourself:    ///
    /// ```no_run
    /// # use windows_ioring_sys::IoRing;
    /// # use std::os::windows::io::AsRawHandle;
    /// # fn f(ring: &mut IoRing) -> std::io::Result<()> {
    /// let event = ring.completion_event()?;
    /// // ... build the wait object around `event`, then arm it ...
    /// // Only now raise the wakeup, in the documented order. A wake with
    /// // nothing to pop is normal (rule 2), so this is always safe.
    /// unsafe { windows_sys::Win32::System::Threading::SetEvent(event.as_raw_handle()) };
    /// # Ok(())
    /// # }
    /// ```
    ///
    #[cfg_attr(
        feature = "threadpool",
        doc = "[`EventDelivery`](crate::EventDelivery) already does this for you and"
    )]
    #[cfg_attr(
        not(feature = "threadpool"),
        doc = "`EventDelivery` (the default `threadpool` feature) already does this for you and"
    )]
    /// is the better answer if you do not need the handle itself.
    ///
    /// `examples/model_b_multiplexed.rs` is this whole shape worked end to
    /// end -- a caller-owned ring waited on alongside a shutdown latch, with
    /// the quiesce that shutdown-while-outstanding requires (M11.6).
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::Unsupported`] if the running system does not
    /// report `IORING_FEATURE_SET_COMPLETION_EVENT`; this crate refuses to
    /// silently substitute a polling thread, since a caller who asked for an
    /// event and got a spun-up thread has been told something false. Also
    /// returns any error from `CreateEventW`,
    /// `SetIoRingCompletionEvent`, `SetEvent`, or duplicating the handle.
    pub fn completion_event(&mut self) -> io::Result<OwnedHandle> {
        let event = self.attach_completion_event_unsignalled()?;
        self.raise_setup_signal()?;
        Ok(event)
    }

    /// Attach the ring's completion event *without* raising the setup signal,
    /// reporting whether that signal is still owed.
    ///
    /// `SetThreadpoolWait` documents that "you must re-register the event with
    /// the wait object before signaling it each time to trigger the wait
    /// callback". A caller that is about to arm a threadpool wait on this
    /// event therefore needs the attachment and the signal as two steps, so
    /// that arming can be sequenced between them; handing back an
    /// already-signalled event leaves that caller no way to obey the rule.
    /// [`IoRing::completion_event`] is these two composed, for a caller who
    /// does its own waiting and is not bound by that rule.
    ///
    /// The caller raises that signal **unconditionally**, whether or not this
    /// call was the one that attached the event. The signal exists to make an
    /// already-present backlog reachable, which is a property of the *waiter
    /// about to arm* rather than of whoever attached the event first: a
    /// caller that attaches, consumes the signal it raised, submits work, and
    /// only then hands the ring to a waiter leaves a non-empty queue that the
    /// edge rule (D-19) will never signal again. Owing the signal only to the
    /// attaching call stranded exactly that backlog permanently, which is
    /// `M26.12`.
    pub(crate) fn attach_completion_event_unsignalled(&mut self) -> io::Result<OwnedHandle> {
        // Already attached: hand back another duplicate rather than
        // attaching a second event, which would silently detach the first
        // (`SetIoRingCompletionEvent` replaces rather than adds). The
        // capability was necessarily verified on the call that attached it.
        if let Some(event) = &self.completion_event {
            return event.try_clone();
        }

        if !capabilities()?.supports_completion_event {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "this system's IoRing does not report IORING_FEATURE_SET_COMPLETION_EVENT",
            ));
        }

        // Auto-reset (manual_reset = FALSE) per D-21, initially unsignalled
        // -- the deliberate setup signal is raised by `raise_setup_signal`,
        // after the event is attached and owned, so it cannot be lost, and
        // after any threadpool wait has been armed, so the arm-before-signal
        // rule `SetThreadpoolWait` documents is obeyed.
        // SAFETY: null attributes and name are documented defaults.
        let raw = unsafe { CreateEventW(std::ptr::null(), 0, 0, std::ptr::null()) };
        if raw.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `CreateEventW` just returned this handle and it is not
        // owned anywhere else, so `OwnedHandle` is its sole owner from here.
        let event = unsafe { OwnedHandle::from_raw_handle(raw) };

        // SAFETY: `self.handle` is a live ring; `event` is a live event that
        // this ring will own for the rest of its life once stored below.
        let hr = unsafe { crate::sys::set_completion_event(self.handle, event.as_raw_handle()) };
        // On failure `event` drops here, closing a handle the ring never
        // successfully referenced.
        check(hr)?;

        // Stored *before* the setup signal can be raised: from this point the
        // ring owns the event, so no later failure can drop it and leave the
        // ring signalling a closed (possibly recycled) handle.
        self.completion_event.insert(event).try_clone()
    }

    /// Raise the one deliberate setup signal on the attached completion event.
    ///
    /// This is the single spurious wakeup the event's contract allows for: a
    /// caller who submitted before attaching, **or who is about to start
    /// waiting on an event someone else attached**, would otherwise never be
    /// woken for that backlog, since the queue never returns to empty and so
    /// never re-arms the edge (D-19). It is therefore raised for every such
    /// caller rather than only for the one that performed the attachment --
    /// see [`IoRing::attach_completion_event_unsignalled`] and `M26.12`.
    ///
    /// Separated from the attachment so that a caller arming a threadpool wait
    /// can obey `SetThreadpoolWait`'s documented ordering -- register first,
    /// signal second. Does nothing if no event is attached, which cannot
    /// happen on the paths that call it.
    pub(crate) fn raise_setup_signal(&self) -> io::Result<()> {
        let Some(event) = &self.completion_event else {
            return Ok(());
        };
        // SAFETY: `event` is a live event handle this ring owns.
        if unsafe { SetEvent(event.as_raw_handle()) } == 0 {
            return Err(io::Error::last_os_error());
        }
        // The setup signal is what a waiter attaching to a backlog depends on
        // entirely, so M26.9's investigation needs to know it happened -- and
        // that it happened on the ring's own handle rather than a duplicate
        // handed to a caller, since only the former is what the kernel will go
        // on signalling.
        //
        // Gated because `windows-threadpool-sys` is optional (D-22): this
        // method is on the always-present path, unlike the delivery module,
        // so an ungated reference breaks `--no-default-features`.
        #[cfg(feature = "threadpool")]
        windows_threadpool_sys::trace_record!(
            "delivery",
            "setup-signalled",
            event.as_raw_handle() as usize,
            self.accounting.outstanding()
        );
        Ok(())
    }

    /// Whether this ring supports a raw op code, including one this crate
    /// does not yet name (D-7).
    ///
    /// Its reason to exist is an op outside [`Op`], which by definition this
    /// ring's cached capability set was never probed for -- but passing a
    /// named op's [`Op::code`] is equally in contract, and answers
    /// identically to [`IoRing::supports`] (M10.1). The difference between
    /// them is cost, not truth: `supports` is a bit test against the set
    /// probed once at construction, this is an `IsIoRingOpSupported` call
    /// every time.
    ///
    /// What a `true` here does *not* mean is that the op became pushable: an
    /// op outside [`Op`] has no builder method whatever this answers, so
    /// [`IoRing::push_raw`] remains the only route to one.
    #[must_use]
    pub fn supports_raw(&self, op_code: IORING_OP_CODE) -> bool {
        // SAFETY: `self.handle` is a live ring.
        unsafe { IsIoRingOpSupported(self.handle, op_code) != 0 }
    }

    /// How many file handles this ring has **reserved** for registration --
    /// not how many are confirmed registered (M5.1, M10.3, D-31).
    ///
    /// The count advances the instant a `BuildIoRingRegisterFileHandles`
    /// call queues, never when its completion is observed. Two consequences
    /// a caller must not be surprised by:
    ///
    /// - it is already advanced before any completion has been popped, so it
    ///   cannot be used to decide whether a registration has taken effect --
    ///   claim the completion with
    ///   [`crate::PendingFileRegistration::claim_if`] for that;
    /// - it stays advanced after a registration whose completion reported
    ///   *failure*, which is why such a registration cannot be retried on
    ///   this ring ([`crate::Batch::register_files`]).
    ///
    /// Because a ring accepts at most one registration that assigns an
    /// index, this is `0` until that registration is queued and its count
    /// thereafter; there is no second registration for it to serve as a base
    /// index for.
    #[must_use]
    pub fn registered_file_count(&self) -> u32 {
        self.accounting.registered_file_count()
    }

    /// As [`IoRing::registered_file_count`], for registered buffers (M5.2) --
    /// a **reserved** count, not a confirmed one, with the same two
    /// consequences (M10.3, D-31).
    #[must_use]
    pub fn registered_buffer_count(&self) -> u32 {
        self.accounting.registered_buffer_count()
    }

    /// Advance the registered-file base index by `count`, the instant a
    /// `BuildIoRingRegisterFileHandles` call successfully queues (not once
    /// its completion is observed).
    ///
    /// D-14 recorded this as an explicitly unverified assumption, since this
    /// crate cannot know whether the kernel claims these `count` indices
    /// synchronously at build time or only once the registration op runs.
    /// D-31 (M10.3) dissolved that: the collision it guarded against needs a
    /// *second* registration, and `Batch::register_files`/`register_buffers`
    /// forbid one, so no later base index is ever derived from this count and
    /// the kernel's actual timing has no observable consequence. What the
    /// eager advance does still determine is the *meaning* of the public
    /// accessors, which is why they document a reserved rather than a
    /// confirmed count.
    ///
    /// D-32 did not answer this question, despite being adjacent to it: it
    /// established when the kernel reads the `IORING_BUFFER_INFO` *array*,
    /// which is a different thing from when it claims the *indices*. The
    /// latter remains unmeasured, and dissolved rather than resolved.
    pub(crate) fn reserve_registered_files(&mut self, count: u32) {
        self.accounting.reserve_registered_files(count);
    }

    /// As [`IoRing::reserve_registered_files`], for registered buffers.
    pub(crate) fn reserve_registered_buffers(&mut self, count: u32) {
        self.accounting.reserve_registered_buffers(count);
    }

    /// Take ownership of the `IORING_BUFFER_INFO` array a
    /// `BuildIoRingRegisterBuffers` call is about to be handed, keeping it
    /// alive until the ring can prove the op has run, and hand back a stable
    /// pointer to it (D-32).
    ///
    /// The kernel reads this array when the registration op *runs*, not when
    /// the `Build*` call returns, so the caller must not build the SQE from a
    /// temporary: store the array here first and pass the returned pointer.
    /// For an empty array the pointer is dangling, as `Vec::as_ptr` is for
    /// any empty `Vec`, and is never dereferenced: the count passed beside it
    /// is zero.
    ///
    /// If the `Build*` call then fails, call
    /// [`IoRing::release_unqueued_buffer_infos`]: nothing was queued, and a
    /// retry needs to hold its own array.
    pub(crate) fn hold_registered_buffer_infos(
        &mut self,
        infos: Vec<IORING_BUFFER_INFO>,
    ) -> *const IORING_BUFFER_INFO {
        debug_assert!(
            self.late_read.buffer_infos.is_empty(),
            "a ring accepts at most one buffer registration, so this must only be set once"
        );
        self.late_read.buffer_infos = infos;
        // `Vec::as_ptr` is stable for as long as the `Vec` is neither moved
        // out of nor reallocated; it lives in `self` and is never mutated
        // again, and moving the `IoRing` itself moves only the `Vec` header,
        // not its heap allocation.
        self.late_read.buffer_infos.as_ptr()
    }

    /// As [`IoRing::hold_registered_buffer_infos`], for the handle array a
    /// `BuildIoRingRegisterFileHandles` call is about to be handed.
    ///
    /// The kernel reads this one late too (D-32). Copied in rather than
    /// borrowed from the caller, so a caller's slice only has to live for
    /// the `register_files` call.
    pub(crate) fn hold_registered_file_handles(
        &mut self,
        handles: Vec<*mut c_void>,
    ) -> *const *mut c_void {
        debug_assert!(
            self.late_read.file_handles.is_empty(),
            "a ring accepts at most one file registration, so this must only be set once"
        );
        self.late_read.file_handles = handles;
        // Stable for the reason given in `hold_registered_buffer_infos`.
        self.late_read.file_handles.as_ptr()
    }

    /// Release the buffer array held for a `BuildIoRingRegisterBuffers` call
    /// that then failed.
    ///
    /// A failed `Build*` queues no SQE, so the kernel will never read the
    /// array, and the documented answer to a full queue is to submit and
    /// retry -- which holds a fresh array and would otherwise trip the
    /// set-once assertion above.
    pub(crate) fn release_unqueued_buffer_infos(&mut self) {
        self.late_read.buffer_infos = Vec::new();
    }

    /// As [`IoRing::release_unqueued_buffer_infos`], for the handle array.
    pub(crate) fn release_unqueued_file_handles(&mut self) {
        self.late_read.file_handles = Vec::new();
    }

    /// How many operations this ring believes are still outstanding: minted
    /// (via `reserve_user_data`) but not yet observed to have completed (via
    /// `record_completion`, which retires only an identity this ring minted).
    #[must_use]
    pub fn outstanding(&self) -> usize {
        self.accounting.outstanding()
    }

    /// Mint a fresh `UserData` identity for a new operation, and account for
    /// it as outstanding until `record_completion` or `cancel_reservation` is
    /// called with it.
    ///
    /// The identity is the whole of what the inventory needs to validate
    /// a completion (D-4): unlike `windows-overlapped-io-sys`'s
    /// `OperationId`, there is no separate storage address to pair it with,
    /// because `UserData` is a value this crate chooses rather than one Win32
    /// hands back.
    ///
    /// # Errors
    ///
    /// Returns an error rather than reusing an identity if the `usize` space
    /// is ever exhausted, mirroring `windows-threadpool-sys`'s own
    /// "exhausting the generation sequence fails rather than wraps."
    pub(crate) fn reserve_user_data(&mut self) -> io::Result<usize> {
        self.accounting.reserve_user_data()
    }

    /// Record that a completion carrying `user_data` has been observed (a real
    /// `IORING_CQE` was popped), whether or not the ring was still holding
    /// something for it. Returns whether `user_data` was outstanding; a
    /// foreign or duplicate completion retires nothing.
    pub(crate) fn record_completion(&mut self, user_data: usize) -> bool {
        self.accounting.record_completion(user_data)
    }

    /// Release a reservation for an operation that was never actually
    /// queued -- a `Build*` call failed synchronously, after
    /// [`IoRing::reserve_user_data`] had already minted its identity.
    ///
    /// Distinct from [`IoRing::record_completion`]: that marks a real
    /// `IORING_CQE` observed; this marks one that will never arrive because
    /// the op never entered the queue, so it must not count against
    /// [`IoRing::run_down`] either.
    pub(crate) fn cancel_reservation(&mut self, user_data: usize) {
        self.accounting.cancel_reservation(user_data);
    }

    /// This ring's ledger, for the crate's own minting paths (M24.2).
    ///
    /// Handed out rather than proxied so that an [`OperationId`] can be minted
    /// from the bookkeeping alone -- which is what lets `token.rs`'s tests run
    /// without a kernel ring, since minting is all they ever needed one for.
    pub(crate) fn accounting_mut(&mut self) -> &mut Accounting {
        &mut self.accounting
    }

    /// This ring's native handle, for `batch.rs`'s `Build*`/`Submit` calls.
    pub(crate) fn raw_handle(&self) -> *mut c_void {
        self.handle
    }

    /// This ring's own identity, for stamping onto every [`crate::OperationId`]/
    /// registration it mints and checking against on use (PR #20 review
    /// response); see [`RingId`].
    pub(crate) fn ring_id(&self) -> RingId {
        self.accounting.ring_id()
    }

    /// Queue a raw, not-yet-wrapped SQE via a caller-supplied `Build*` call
    /// (M3.5, D-7).
    ///
    /// `build` receives this ring's native handle and a freshly reserved
    /// `UserData` value, and must call exactly one `BuildIoRing*` function
    /// with them, returning its `HRESULT`. On success the `UserData` is
    /// returned so the caller can match it against a later [`Completion`]
    /// popped by [`IoRing::try_pop`]; on failure the reservation is
    /// released, since the op was never actually queued.
    ///
    /// **Matching by hand is correct here, and only here.** This seam creates
    /// no inventory entry -- it cannot, since it does not know what the
    /// caller's `build` closure queued or what it needs held -- so a pop for
    /// this operation reports `None` for what the ring was holding. That is
    /// the legitimate half of [D-75](../../DESIGN-NOTES.md#d-75)'s two causes,
    /// and choosing this seam is what makes it legitimate. Every other push
    /// hands its payload to the ring and gets it back from the pop.
    ///
    /// # Errors
    ///
    /// Returns any error `build`'s `HRESULT` reports.
    ///
    /// # Safety
    ///
    /// `build` must queue an SQE for a *self-contained* op: everything the
    /// kernel reads or writes for it must stay valid until the
    /// corresponding completion is observed, and this crate cannot verify
    /// what `build` does with the handle it is given. This is the same
    /// framing as `windows-overlapped-io-sys`'s raw `ioctl` seam
    /// (`device.rs`): the mechanics of building an SQE need nothing unsafe,
    /// but this crate cannot audit an arbitrary `Build*` call, so the seam
    /// itself is unsafe.
    ///
    /// `build` must also queue its SQE with **exactly the `user_data` it was
    /// handed**, and queue only one. A completion carrying any other identity
    /// is one this ring never minted, and the pop that receives it panics
    /// ([D-79](../../DESIGN-NOTES.md#d-79)).
    pub unsafe fn push_raw(
        &mut self,
        build: impl FnOnce(*mut c_void, usize) -> windows_sys::core::HRESULT,
    ) -> io::Result<usize> {
        let user_data = self.reserve_user_data()?;
        let hr = build(self.handle, user_data);
        if let Err(error) = check(hr) {
            self.cancel_reservation(user_data);
            return Err(error);
        }
        Ok(user_data)
    }

    /// Block until every outstanding operation has completed, so
    /// `CloseIoRing` never runs while the kernel might still be touching a
    /// buffer the ring holds for an in-flight operation.
    ///
    /// Waits in short, rechecked steps via `SubmitIoRing`'s own wait -- with
    /// zero new entries queued, its only effect is to block for up to
    /// `RUN_DOWN_POLL_MS` and reap whatever is already outstanding -- rather
    /// than one unbounded call. This does not interpret what it pops: each
    /// popped entry's payload is dropped, which is sound because its
    /// completion is the proof the kernel has finished with it. Idempotent: calling it
    /// again once `outstanding() == 0` is a no-op.
    ///
    /// # A poll that expires is not a failure
    ///
    /// Each poll blocks for `RUN_DOWN_POLL_MS` and then reports
    /// `ERROR_TIMEOUT` if nothing finished in that window, which is the
    /// ordinary outcome for any operation slower than 50ms. Treating that as
    /// an error -- which this did until M21.6 -- made `run_down` return `Err`
    /// with the operation still outstanding, so `Drop` asserted and then
    /// called `CloseIoRing` anyway: exactly the "the kernel may still be
    /// writing through a buffer the ring holds" hazard this function exists to
    /// prevent.
    ///
    /// This loop therefore has no overall bound, and that is deliberate.
    /// **Every SQE that successfully queues produces exactly one completion**
    /// (M10.2), so it terminates. Blocking until that holds is the safe
    /// failure mode; closing the ring early is not.
    ///
    /// # Choosing an unbounded wait is the caller's to make
    ///
    /// This spelling waits until rundown finishes, however long that takes,
    /// and calling it is how a caller elects that. A caller who wants to
    /// decide for themselves -- a deadline, a backoff, a number of attempts
    /// before giving up -- calls [`IoRing::run_down_within`] instead and owns
    /// the policy entirely. **This crate does not implement retry policy**
    /// (M26.8): it supplies a bounded primitive and reports what happened.
    ///
    /// Note the difference from [`IoRing::pop_within`], which also waits in
    /// segments: there the segments sit *inside a period the caller supplied*,
    /// which is a bounded wait implemented properly rather than a policy. This
    /// method had segments and no such period, which is what made it the one
    /// waiting API in this crate shaped wrongly.
    ///
    /// # Errors
    ///
    /// Returns any error from `SubmitIoRing` other than an expired wait, or
    /// from `PopIoRingCompletion`. **An error leaves the ring resumable**: see
    /// [`IoRing::run_down_within`] for what is guaranteed about the operations
    /// still queued.
    ///
    /// # Panics
    ///
    /// As [`IoRing::try_pop`], on a completion for an identity not in flight.
    pub fn run_down(&mut self) -> io::Result<()> {
        while !self.run_down_within(Duration::MAX)? {}
        Ok(())
    }

    /// Run down for at most `bound`, reporting whether it finished.
    ///
    /// `Ok(true)` means nothing is outstanding and the ring is safe to drop.
    /// `Ok(false)` means the bound elapsed with work still in flight -- call
    /// again when your own policy says to. [`IoRing::outstanding`] says how
    /// much is left.
    ///
    /// # Why this exists, and why it returns rather than retries
    ///
    /// Rundown can fail for a reason that a later attempt would survive, and
    /// **deciding whether to make that attempt is not this crate's business**.
    /// A caller running under a deadline, a supervisor with a backoff, and a
    /// test that wants to fail fast all want different answers, and a policy
    /// baked in here would be wrong for two of the three. So this waits for
    /// exactly as long as it is told and then reports.
    ///
    /// # What an error guarantees, which is what makes retrying safe
    ///
    /// `SubmitIoRing` documents that *"If this function returns an error other
    /// than IORING_E_WAIT_TIMEOUT, then all entries remain in the submission
    /// queue."* So a failure here has **not** lost the operations and has not
    /// rewound them ([D-5](../../DESIGN-NOTES.md#d-5)); they are still ring state,
    /// a later submit is what runs them, and their buffers must stay alive
    /// until they complete.
    ///
    /// The consequence worth stating plainly: after an `Err`, **do not drop
    /// this ring**. Dropping it closes a ring the kernel may still write
    /// through, which is the hazard rundown exists to prevent. Call again.
    ///
    /// # Errors
    ///
    /// Returns any error from `SubmitIoRing` other than an expired wait, or
    /// from `PopIoRingCompletion`.
    ///
    /// # Panics
    ///
    /// As [`IoRing::try_pop`], on a completion for an identity not in flight.
    pub fn run_down_within(&mut self, bound: Duration) -> io::Result<bool> {
        let deadline = Instant::now().checked_add(bound);
        loop {
            if self.is_quiescent() {
                return Ok(true);
            }
            // `is_quiescent` waits on IDENTITIES, not a count. With A and B
            // outstanding, a foreign CQE plus A's real one retires only A, so
            // this keeps waiting for B -- and B's will arrive, because every SQE
            // that queues produces exactly one completion (M10.2). That is the
            // invariant that already justifies this loop having no overall
            // bound, and it holds for raw pushes, which stow nothing, exactly as
            // for owned ones.
            // `checked_add` rather than `+`, for the reason `pop_within_with`
            // records: `Instant + Duration` panics on overflow, so
            // `Duration::MAX` -- the honest spelling of "no deadline" -- would
            // take down the process. A deadline the clock cannot represent is
            // one that never arrives, which is what was asked for.
            let remaining = match deadline {
                Some(deadline) => deadline.saturating_duration_since(Instant::now()),
                None => Duration::MAX,
            };
            if remaining.is_zero() {
                return Ok(false);
            }
            // Segments sit inside the caller's period, never outside it: the
            // poll is the shorter of the rundown step and what is left.
            let poll_ms = u32::try_from(remaining.as_millis())
                .unwrap_or(u32::MAX)
                .clamp(1, RUN_DOWN_POLL_MS);
            let mut submitted = 0_u32;
            // SAFETY: `self.handle` is a live ring; valid out-pointer. Zero
            // new SQEs are queued -- this call's only purpose is to wait for
            // and reap already-outstanding completions.
            let hr = unsafe { crate::sys::submit(self.handle, 1, poll_ms, &raw mut submitted) };
            wait_outcome(hr)?;
            self.drain_for_rundown()?;
        }
    }
}
