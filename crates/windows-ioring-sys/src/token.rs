// Copyright (c) 2026 Mike Grier
//! `Token<B>`: an owned buffer bound to one in-flight operation (M2.2, M2.3).

use std::mem::ManuallyDrop;

use crate::buf::IoBuf;

/// An owned buffer, plus the identity of the operation the kernel may still
/// be reading or writing it through.
///
/// `Token` intentionally holds no reference back to the [`crate::IoRing`]
/// that minted its identity: outstanding-operation accounting belongs to the
/// ring (D-4 in `DESIGN-NOTES.md`), driven by whoever actually pops an
/// `IORING_CQE`, not by a `Token` merely being dropped. A dropped, unclaimed
/// `Token` means the *caller* has given up on this handle; it says nothing
/// about whether the kernel has finished the operation, which is exactly why
/// dropping it forgets the buffer instead of freeing it -- see
/// [`Token`]'s `Drop` impl.
pub struct Token<B: IoBuf> {
    id: usize,
    buffer: ManuallyDrop<B>,
}

impl<B: IoBuf> Token<B> {
    /// Wrap `buffer` under a fresh identity reserved from `ring`.
    ///
    /// # Errors
    ///
    /// Returns any error from [`crate::IoRing::reserve_user_data`] (in
    /// practice, only if the identity space is exhausted).
    pub(crate) fn new(ring: &mut crate::IoRing, buffer: B) -> std::io::Result<Self> {
        let id = ring.reserve_user_data()?;
        Ok(Self {
            id,
            buffer: ManuallyDrop::new(buffer),
        })
    }

    /// This token's `UserData` identity.
    #[must_use]
    pub fn id(&self) -> usize {
        self.id
    }

    /// Consume this token and recover its buffer unconditionally.
    ///
    /// Only for a caller who already knows the operation completed (for
    /// example, [`Token::claim_if`] on a match). Not `pub`: calling this
    /// without that knowledge is exactly the use-after-free this type exists
    /// to prevent.
    fn claim(mut self) -> B {
        // SAFETY: `self` is not used again after this -- it is dropped
        // normally by the caller's scope immediately after, and `Token`'s own
        // `Drop` never reads `buffer` (see that impl), so taking it here
        // leaves nothing for `Drop` to double-free.
        unsafe { ManuallyDrop::take(&mut self.buffer) }
    }

    /// Claim this token's buffer if `user_data` names it, or hand it back
    /// unchanged otherwise (D-4).
    ///
    /// This is the whole validation a completion needs here: unlike
    /// `windows-overlapped-io-sys`'s `OperationId`, there is no storage
    /// address to also check, because `UserData` is not an address -- it is
    /// a value this crate chose, so a stale token's `id` can never
    /// coincidentally match a different, later operation's completion the
    /// way a reused memory address could.
    pub fn claim_if(self, user_data: usize) -> Result<B, Self> {
        if self.id == user_data {
            Ok(self.claim())
        } else {
            Err(self)
        }
    }
}

impl<B: IoBuf> Drop for Token<B> {
    fn drop(&mut self) {
        // Deliberately empty. `buffer` is a `ManuallyDrop<B>`, which already
        // never runs `B`'s destructor on its own; declaring this impl (rather
        // than omitting it and relying on that) makes the leak-not-free
        // choice a visible, intentional part of this type rather than an
        // accident of its field types (D-4).
    }
}

impl<B: IoBuf> std::fmt::Debug for Token<B> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Not derived: deriving would require `B: Debug`, and a caller's
        // buffer type need not implement it. The id is the only part of a
        // token useful to print anyway.
        f.debug_struct("Token")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests;
