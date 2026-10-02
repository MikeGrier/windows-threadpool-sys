// Copyright (c) 2026 Mike Grier
//! The bookkeeping half of a ring, with no kernel state in it (M24.2).
//!
//! # Why this is a separate type
//!
//! [`crate::IoRing`] had ten fields and they divide evenly. Five name kernel
//! state -- the handle, the negotiated version, the probed op support, the
//! registration array the kernel reads late (`D-32`), and the completion event.
//! The other five are a ledger this crate keeps for itself: the ring's
//! identity, the next `UserData` to hand out, how many operations are
//! outstanding, and the two registration base indices.
//!
//! Nothing in that second half needs a ring to exist. It is arithmetic and
//! identity, and the rules it enforces -- that an identity is never reused,
//! that a reservation is released exactly once, that the counters saturate
//! rather than wrap -- are **this crate's own specification**, not anything
//! Windows has an opinion about.
//!
//! Splitting it out is what lets those rules be tested without opening a
//! kernel ring, which is [D-49](../DESIGN-NOTES.md#d-49)'s defect: `cargo test
//! --lib` did not mean what its name implies. This type needs no fake, no
//! feature gate and no widened visibility to be exercised exhaustively.
//!
//! # What it deliberately does not know
//!
//! Whether an operation actually reached the kernel, whether a completion is
//! real, or whether the handle is still open. [`Accounting`] records what it
//! is *told*; the ring is what talks to Windows. Keeping that line sharp is
//! the point -- a ledger that tried to second-guess the kernel would be the
//! mock this crate rejects ([D-52](../DESIGN-NOTES.md#d-52)).

use std::io;
use std::sync::atomic::{AtomicU64, Ordering};

/// A ring's identity, unique for the process's lifetime (PR #20 review
/// response): every value a ring hands out that later gets checked back
/// against it -- an inventory entry, a [`crate::RegisteredFile`], a
/// [`crate::RegisteredBuffers`] -- carries the id of the ring that minted
/// it, and every [`crate::Completion`] carries the id of the ring that
/// popped it.
///
/// A monotonic counter rather than the ring's own `HANDLE`: a `HANDLE` is
/// only unique while the object it names is still open, and Windows is free
/// to hand a closed ring's numeric value to the *next* object created --
/// which would let a stale identity from a closed ring collide with a
/// brand-new one. This counter never repeats within one process run
/// (`u64` overflow is not a practical concern), so a mismatch always means
/// a genuine cross-ring mixup, never a false negative from handle reuse.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct RingId(u64);

impl RingId {
    /// The next identity. Process-global and monotonic.
    ///
    /// `Relaxed` is sufficient: the only property required is that no two
    /// calls return the same value, which `fetch_add` gives on its own. No
    /// other memory is being published through this counter.
    pub(crate) fn next() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

/// A ring's ledger: identity, operation identities, and the counts.
#[derive(Debug)]
pub(crate) struct Accounting {
    ring_id: RingId,
    /// The next `UserData` value [`Accounting::reserve_user_data`] will hand
    /// out.
    next_user_data: usize,
    /// Operations minted but not yet observed to have completed (M2.4).
    outstanding: usize,
    /// How many file handles are registered so far, across every confirmed
    /// `BuildIoRingRegisterFileHandles` (M5.1). The base index of the next
    /// registration.
    registered_files: u32,
    /// As `registered_files`, for `BuildIoRingRegisterBuffers` (M5.2).
    registered_buffers: u32,
}

impl Accounting {
    /// A fresh ledger, with an identity no other ring in this process holds.
    pub(crate) fn new() -> Self {
        Self {
            ring_id: RingId::next(),
            next_user_data: 0,
            outstanding: 0,
            registered_files: 0,
            registered_buffers: 0,
        }
    }

    /// This ring's own identity, for stamping onto every [`crate::OperationId`] and
    /// registration it mints and checking against on use.
    pub(crate) fn ring_id(&self) -> RingId {
        self.ring_id
    }

    /// How many operations this ring believes are still outstanding: minted
    /// (via [`Accounting::reserve_user_data`]) but not yet observed to have
    /// completed (via [`Accounting::record_completion`]).
    pub(crate) fn outstanding(&self) -> usize {
        self.outstanding
    }

    /// Mint a fresh `UserData` identity for a new operation, and account for
    /// it as outstanding until `record_completion` is called for it.
    ///
    /// # Errors
    ///
    /// Returns an error rather than reusing an identity if the `usize` space
    /// is ever exhausted, mirroring `windows-threadpool-sys`'s own
    /// "exhausting the generation sequence fails rather than wraps."
    pub(crate) fn reserve_user_data(&mut self) -> io::Result<usize> {
        let id = self.next_user_data;
        self.next_user_data = id
            .checked_add(1)
            .ok_or_else(|| io::Error::other("IoRing operation identity space exhausted"))?;
        self.outstanding += 1;
        Ok(id)
    }

    /// Record that one outstanding operation's completion has been observed
    /// (a real `IORING_CQE` was popped for it), whether or not a live
    /// the ring was still holding something for it.
    pub(crate) fn record_completion(&mut self) {
        self.outstanding = self.outstanding.saturating_sub(1);
    }

    /// Release a reservation for an operation that was never actually
    /// queued -- a `Build*` call failed synchronously, after
    /// [`Accounting::reserve_user_data`] had already minted its identity.
    ///
    /// Distinct from [`Accounting::record_completion`]: that marks a real
    /// `IORING_CQE` observed; this marks one that will never arrive because
    /// the op never entered the queue, so it must not count against
    /// `IoRing::run_down` either.
    ///
    /// The two are separate methods rather than one, even though their bodies
    /// are identical, because they record different *facts* and a future
    /// change to either -- a conservation counter, a debug assertion -- is
    /// overwhelmingly likely to apply to only one of them.
    pub(crate) fn cancel_reservation(&mut self) {
        self.outstanding = self.outstanding.saturating_sub(1);
    }

    /// The registered-file base index: how many file handles are registered
    /// so far.
    pub(crate) fn registered_file_count(&self) -> u32 {
        self.registered_files
    }

    /// As [`Accounting::registered_file_count`], for registered buffers.
    pub(crate) fn registered_buffer_count(&self) -> u32 {
        self.registered_buffers
    }

    /// Advance the registered-file base index by `count`, the instant a
    /// `BuildIoRingRegisterFileHandles` call successfully queues (not once
    /// its completion is observed).
    pub(crate) fn reserve_registered_files(&mut self, count: u32) {
        self.registered_files = self.registered_files.saturating_add(count);
    }

    /// As [`Accounting::reserve_registered_files`], for registered buffers.
    pub(crate) fn reserve_registered_buffers(&mut self, count: u32) {
        self.registered_buffers = self.registered_buffers.saturating_add(count);
    }

    /// Place the identity counter near its ceiling, so the exhaustion path is
    /// reachable.
    ///
    /// `#[cfg(test)]`, so it does not ship. It exists because that path is
    /// otherwise unreachable -- a real ring would have to mint `usize::MAX`
    /// operations -- and the repository's rule is that an error edge no test
    /// can traverse is written rather than implemented. The alternative to
    /// erroring there is silently handing out a duplicate identity, which is
    /// exactly what a `Token` cannot survive.
    #[cfg(test)]
    pub(crate) fn set_next_user_data_for_test(&mut self, next: usize) {
        self.next_user_data = next;
    }
}

#[cfg(test)]
mod tests;
