// Copyright (c) 2026 Mike Grier
//! `Token<T>`: an owned value bound to one in-flight operation (M2.2, M2.3).

use crate::ring::RingId;

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
/// That is the same property `Token::claim_if` bought at runtime, and this
/// buys it structurally. The retired `claim_if` took a
/// [`crate::Completion`] rather than a `usize` precisely because a
/// `Completion` has no public constructor, so the only proof the kernel has
/// finished was one that had been popped; a caller-supplied integer would have
/// let safe code free a buffer the kernel was still writing into. `M28.4.1d.3`
/// removed the call rather than the argument: the payload now comes back *from*
/// the pop, so there is no second entry point to guard.
///
/// Handing an `OperationId` to a cancel stays safe for the reason it was always
/// safe to pass a token's id there: a cancel returns no memory.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct OperationId {
    id: usize,
    ring_id: RingId,
}

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

#[cfg(test)]
mod tests;
