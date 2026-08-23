// Copyright (c) 2026 Mike Grier
//! `Token<T>`: an owned value bound to one in-flight operation (M2.2, M2.3).

use std::mem::ManuallyDrop;

use crate::ring::{Completion, RingId};

/// An owned value, plus the identity of the operation the kernel may still
/// be reading or writing through it.
///
/// Usually `T` is an [`crate::IoBuf`] the operation reads or writes, but
/// nothing about this type depends on that: M5's buffer registration reuses
/// the same forget-unless-claimed mechanism to track a registration's own
/// in-flight uses, where `T` carries no bytes at all (see
/// `RegisteredUse` in `batch.rs`).
///
/// `Token` intentionally holds no reference back to the [`crate::IoRing`]
/// that minted its identity: outstanding-operation accounting belongs to the
/// ring (D-4 in `DESIGN-NOTES.md`), driven by whoever actually pops an
/// `IORING_CQE`, not by a `Token` merely being dropped. A dropped, unclaimed
/// `Token` means the *caller* has given up on this handle; it says nothing
/// about whether the kernel has finished the operation, which is exactly why
/// dropping it forgets the value instead of freeing it -- see
/// [`Token`]'s `Drop` impl.
pub struct Token<T: Send + 'static> {
    id: usize,
    ring_id: RingId,
    value: ManuallyDrop<T>,
}

impl<T: Send + 'static> Token<T> {
    /// Wrap `value` under a fresh identity reserved from `ring`.
    ///
    /// # Errors
    ///
    /// Returns any error from [`crate::IoRing::reserve_user_data`] (in
    /// practice, only if the identity space is exhausted).
    pub(crate) fn new(ring: &mut crate::IoRing, value: T) -> std::io::Result<Self> {
        let id = ring.reserve_user_data()?;
        Ok(Self {
            id,
            ring_id: ring.ring_id(),
            value: ManuallyDrop::new(value),
        })
    }

    /// This token's `UserData` identity.
    #[must_use]
    pub fn id(&self) -> usize {
        self.id
    }

    /// Consume this token and recover its value unconditionally.
    ///
    /// Only for a caller who already knows the operation completed --
    /// [`Token::claim_if`] on a match, or the crate's own abort path for a
    /// push that never queued at all (`Batch::finish_push`). Not `pub`:
    /// calling this without that knowledge is exactly the use-after-free
    /// this type exists to prevent.
    pub(crate) fn claim(mut self) -> T {
        // SAFETY: `self` is not used again after this -- it is dropped
        // normally by the caller's scope immediately after, and `Token`'s own
        // `Drop` never reads `value` (see that impl), so taking it here
        // leaves nothing for `Drop` to double-free.
        unsafe { ManuallyDrop::take(&mut self.value) }
    }

    /// Claim this token's value if `completion` names it, or hand it back
    /// unchanged otherwise (D-4).
    ///
    /// Takes a popped [`Completion`], not a bare `usize`: `Completion` has no
    /// public constructor, so the only way to produce one is
    /// [`crate::IoRing::try_pop`] actually observing a real `IORING_CQE`.
    /// Accepting a caller-supplied integer here instead -- for example
    /// `token.claim_if(token.id())` -- would let safe code reclaim (and then
    /// drop, freeing) a buffer the kernel might still be reading or writing,
    /// which is exactly the use-after-free this type exists to prevent.
    /// Unlike `windows-overlapped-io-sys`'s `OperationId`, there is no
    /// storage address to also check on its own -- `UserData` is a value
    /// this crate chose -- but two different rings each hand out `UserData`
    /// from their own counter starting at zero, so the same value can
    /// legitimately occur on both. Requiring `completion`'s ring identity to
    /// match this token's own (PR #20 review response) is what rules that
    /// out: a token can only ever be claimed by a completion popped from the
    /// exact ring that minted it.
    pub fn claim_if(self, completion: &Completion) -> Result<T, Self> {
        if self.id == completion.user_data() && self.ring_id == completion.ring_id() {
            Ok(self.claim())
        } else {
            Err(self)
        }
    }
}

impl<T: Send + 'static> Drop for Token<T> {
    fn drop(&mut self) {
        // Deliberately empty. `value` is a `ManuallyDrop<T>`, which already
        // never runs `T`'s destructor on its own; declaring this impl (rather
        // than omitting it and relying on that) makes the leak-not-free
        // choice a visible, intentional part of this type rather than an
        // accident of its field types (D-4).
    }
}

impl<T: Send + 'static> std::fmt::Debug for Token<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Not derived: deriving would require `T: Debug`, and a caller's
        // value type need not implement it. The id is the only part of a
        // token useful to print anyway.
        f.debug_struct("Token")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests;
