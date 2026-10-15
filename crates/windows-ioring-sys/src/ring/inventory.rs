// Copyright (c) 2026 Mike Grier
// Split from ring.rs at 265a7a04.
//! The ring-owned inventory: stowing a payload at push and retiring it at the
//! pop that observed its completion.
//!
//! A child of [`super`] rather than a sibling, because Rust's privacy is
//! asymmetric and these call the parent's private `pop_raw`. The quiescence
//! predicate, the pop internals and `Drop` stay up there, where `Drop` can
//! still reach them.

use super::*;

impl<T, X> IoRing<T, X, Open> {
    /// Create a ring whose inventory holds `T`, negotiating the version as
    /// [`IoRing::new`] does.
    ///
    /// The generic counterpart to [`IoRing::new`], which exists separately
    /// only so that the common no-payload call keeps inferring its parameter.
    ///
    /// # One `T` per ring
    ///
    /// `T` is a single type, and a pop returns it without a cast because of
    /// that. A caller whose operations carry buffers of *different* types --
    /// say a [`win_numa_sys::NumaBuffer`] arena alongside ad-hoc `Vec<u8>`
    /// records -- therefore cannot put them through one ring as they stand.
    /// Two ways round it:
    ///
    /// - **A ring per buffer type.** Each keeps its own `T`, and the operations
    ///   are independent at the ring level.
    /// - **An enum payload.** One type with a variant per buffer, carrying its
    ///   own `unsafe impl IoBuf`. The obligation that impl takes on is the same
    ///   one [`IoBuf`](crate::IoBuf) states, and it has to hold for every
    ///   variant: the address must not move while the kernel holds it, which
    ///   for an enum means the bytes must not live inline in the variant.
    ///
    /// Erasing `T` is not among them -- see
    /// [D-4](../../DESIGN-NOTES.md#d-4).
    ///
    /// # Errors
    ///
    /// As [`IoRing::new`].
    pub fn with_inventory(
        submission_queue_size: u32,
        completion_queue_size: u32,
    ) -> io::Result<Self> {
        let caps = capabilities()?;
        let version = RingVersion::HIGHEST_KNOWN.min(caps.max_version);
        Self::with_version_and_inventory(version, submission_queue_size, completion_queue_size)
    }
}

impl<T, X, K> IoRing<T, X, K> {
    /// Record what an operation is holding, under the identity it will
    /// complete with.
    ///
    /// The other half of [`IoRing::reclaim`]. Between them they are the whole
    /// mechanism `D-55` asked for: a consumer never holds a token, so it
    /// cannot lose one, and the map cannot drift from the ring because the
    /// ring *is* the map.
    pub(crate) fn stow(&mut self, id: OperationId, entry: Entry<T, X>) {
        debug_assert_eq!(
            id.ring_id(),
            self.accounting.ring_id(),
            "an identity minted by another ring must never reach this inventory"
        );
        self.inventory.insert(id.user_data(), entry);
    }

    /// Take back what an operation was holding, if this ring was holding
    /// anything for it.
    ///
    /// `None` means a push that created no entry at all -- the `_raw` flush
    /// and cancel forms and `push_raw`, which return a bare `user_data`.
    ///
    /// A completion for an identity this ring never minted cannot reach here
    /// as a quiet `None` any more: the identity ledger notices it first, and
    /// the pop panics ([D-79](../../DESIGN-NOTES.md#d-79)) -- except during an
    /// unwind, where it is traced and this returns `None` for it. `M28.5`
    /// settled that the inventory is the right place to stop: an entry for
    /// every raw push would need an `X` the caller never supplied.
    pub(crate) fn reclaim(&mut self, user_data: usize) -> Option<Entry<T, X>> {
        self.inventory.remove(&user_data)
    }

    /// How many operations this ring is currently holding something for.
    ///
    /// Distinct from [`IoRing::outstanding`], which counts every operation the
    /// *kernel* still owes a completion for. The two are **not** expected to
    /// be equal: every operation held here is also outstanding, but a `_raw`
    /// flush or cancel and [`IoRing::push_raw`] are outstanding while holding
    /// nothing, by design. So `held() <= outstanding()` always, and the
    /// difference is the number of raw operations in flight -- not a sign of
    /// corruption ([D-78](../../DESIGN-NOTES.md#d-78)). On an [`Owned`] ring the
    /// two are equal.
    #[must_use]
    pub fn held(&self) -> usize {
        self.inventory.len()
    }
}

impl<T, X, K: RingKind> IoRing<T, X, K> {
    /// Pop a completion and take back whatever this ring was holding for it.
    ///
    /// The replacement for popping a [`Completion`] and matching it against a
    /// held token (`D-71`). The payload is produced **by this call and by no
    /// other**, which is what makes a use-after-free unrepresentable rather
    /// than merely guarded: there is no way to name an operation and be handed
    /// the memory it may still be using. [`crate::OperationId`] deliberately
    /// cannot do it.
    ///
    /// # What each `None` means
    ///
    /// There are two, and they answer different questions.
    ///
    /// A `Some` whose **payload** is `None` is an operation that never had a
    /// buffer -- a flush or a cancellation pushed through an `_owned` form.
    /// Its sidecar still arrives, which is the point: a flush can say which
    /// group of writes it belonged to.
    ///
    /// The **outer** `None` means this ring is holding nothing for that
    /// identity because the push that produced it created no entry: a `_raw`
    /// flush or cancel, or [`IoRing::push_raw`]. Those return a bare
    /// `user_data` and deliberately create no entry -- choosing one *is*
    /// choosing not to have the ring hold anything. Nothing is wrong. `M28.5`
    /// decided not to give every raw push an entry: that would need an `X` the
    /// caller never supplied, and the `_owned` forms already exist for a
    /// caller who wants one.
    ///
    /// # On an [`Owned`] ring
    ///
    /// Neither case of nothing is possible: no call that queues an operation the ring holds
    /// nothing for exists for it, so the return is the flat `(completion, payload, sidecar)`,
    /// with the payload `None` for an operation that never had a buffer
    /// ([D-91](../../DESIGN-NOTES.md#d-91)).
    ///
    /// # Errors
    ///
    /// Any error from `PopIoRingCompletion`.
    ///
    /// # Panics
    ///
    /// If the completion carries an identity that is not in flight on this
    /// ring -- never minted here, already completed, or released after its
    /// build failed. That is a defect, not an outcome
    /// ([D-79](../../DESIGN-NOTES.md#d-79)); during an unwind it is traced
    /// instead, and the completion is returned with an outer `None`.
    pub fn try_pop(&mut self) -> io::Result<Option<K::Popped<T, X>>> {
        let Some(completion) = self.pop_raw()? else {
            return Ok(None);
        };
        // The outer `Option` is whether this ring stowed anything for that
        // identity; the inner one is whether what it stowed included a buffer.
        // Collapsing the two -- which an earlier version did -- loses the
        // sidecar of every bufferless operation, so a flush could never say
        // which group of writes it belonged to.
        let held = self
            .reclaim(completion.user_data())
            .map(|entry| (entry.payload, entry.extra));
        K::shape(completion, held).map(Some)
    }
}

impl<T, X> IoRing<T, X, Open> {
    /// Seal this ring against operations it holds nothing for, so it can be an [`Owned`]
    /// ring ([D-91](../../DESIGN-NOTES.md#d-91)).
    ///
    /// An [`Open`] ring can have in flight an operation the ring holds nothing for: a
    /// registration, a `_raw` flush or cancel, or [`IoRing::push_raw`]. Those are queued
    /// by calls an [`Owned`] ring does not have, and the ring this returns can never have one
    /// again; the conversion consumes the ring and there is no way back. So it is refused while
    /// any is in flight -- [`IoRing::outstanding`] exceeds [`IoRing::held`] by exactly that
    /// many -- and the ring is handed back, to be popped until it is not.
    ///
    /// Nothing is lost: the ring's identity, its registrations (a [`RegisteredFiles`](crate::RegisteredFiles) and a
    /// `RegisteredBuffers` it made still name it), its completion event and its queue are the
    /// same.
    ///
    /// # Errors
    ///
    /// The ring, unchanged, if an operation it holds nothing for is in flight.
    // The `Err` is the ring itself, handed back so the caller can pop what is in flight and ask
    // again; boxing it would make every caller of a once-per-ring call unbox it.
    #[allow(clippy::result_large_err)]
    pub fn into_owned(self) -> Result<IoRing<T, X, Owned>, Self> {
        if self.outstanding() != self.held() {
            return Err(self);
        }
        let this = ManuallyDrop::new(self);
        // SAFETY: `IoRing` is `repr(C)` and `K` appears in it only as `PhantomData<fn() -> K>`,
        // so `IoRing<T, X, Open>` and `IoRing<T, X, Owned>` have one layout, field for field. The
        // read moves the whole ring out of `this`, which is never dropped, so there is one owner
        // before and after.
        Ok(unsafe {
            std::ptr::read(std::ptr::from_ref::<Self>(&this).cast::<IoRing<T, X, Owned>>())
        })
    }
}
