// Copyright (c) 2026 Mike Grier
//! `Token<T>`: an owned value bound to one in-flight operation (M2.2, M2.3).

use std::mem::ManuallyDrop;

use crate::ring::{Completion, RingId};

/// The name of one in-flight operation, carrying no claim on anything.
///
/// Returned by a push so a caller can *refer* to the operation it just
/// queued -- to cancel it, to correlate it, or to feed
/// [`crate::contract::RingContract`]. It is `Copy`, has no `Drop`, and losing
/// one is harmless.
///
/// # A name, not a capability (D-71)
///
/// This deliberately carries **no power to retrieve a payload**. The ring
/// hands a payload back only from the pop that observed its completion, which
/// is what makes a use-after-free unrepresentable: there is no call that turns
/// an identity into the memory an operation may still be using.
///
/// That is the same property [`Token::claim_if`] bought at runtime and this
/// buys structurally. `claim_if` takes a [`Completion`] rather than a `usize`
/// precisely because `Completion` has no public constructor, so the only proof
/// the kernel has finished is one that was popped; accepting a caller-supplied
/// integer would let safe code free a buffer the kernel is still writing into.
/// Handing an `OperationId` to a cancel stays safe for the same reason it was
/// always safe to pass `Token::id` there: a cancel returns no memory.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct OperationId {
    id: usize,
    ring_id: RingId,
}

// `expect` rather than `allow`, deliberately: nothing mints an `OperationId`
// until `M28.3.3` wires the inventory, and when it does this attribute starts
// warning on its own. Scaffolding that removes itself beats scaffolding that
// needs remembering.
#[expect(
    dead_code,
    reason = "minted by the inventory push that M28.3.3 adds; see M28.3 in CHECKLIST.md"
)]
impl OperationId {
    pub(crate) fn new(id: usize, ring_id: RingId) -> Self {
        Self { id, ring_id }
    }

    /// The `UserData` value this operation carries.
    ///
    /// The integer the kernel echoes back in its `IORING_CQE`. Useful as a
    /// cancel target and for a caller's own correlation; it is not a handle to
    /// anything.
    #[must_use]
    pub fn user_data(&self) -> usize {
        self.id
    }

    /// The ring that minted this identity.
    ///
    /// Every ring hands out `UserData` from its own counter starting at the
    /// same value, so the same integer legitimately occurs on two rings. The
    /// inventory is per-ring, which makes that collision structurally
    /// harmless -- but a caller correlating across rings needs this to tell
    /// them apart.
    pub(crate) fn ring_id(&self) -> RingId {
        self.ring_id
    }
}

impl std::fmt::Display for OperationId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:#x}", self.id)
    }
}

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
    /// Wrap `value` under a fresh identity reserved from `accounting`.
    ///
    /// Takes the ring's **ledger** rather than the ring (M24.7). Minting
    /// needs an identity and the ring's id, both of which are bookkeeping --
    /// no handle is involved -- and narrowing the parameter to what is
    /// actually used is what lets this be exercised without opening a ring.
    ///
    /// # Errors
    ///
    /// Returns any error from [`crate::accounting::Accounting::reserve_user_data`]
    /// (in practice, only if the identity space is exhausted).
    pub(crate) fn new(
        accounting: &mut crate::accounting::Accounting,
        value: T,
    ) -> std::io::Result<Self> {
        let id = accounting.reserve_user_data()?;
        Ok(Self {
            id,
            ring_id: accounting.ring_id(),
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
