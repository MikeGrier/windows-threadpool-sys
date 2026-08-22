// Copyright (c) 2026 Mike Grier
//! The outcome of a buffer-owning adapter's submission.
//!
//! An adapter (`fs::read`, `device::ioctl`, `socket::send`, ...) owns the buffers
//! an operation reads into or writes from, so it can report one of exactly two
//! things once the native call returns: the operation is in flight and its
//! result must be claimed from a completion later, or it is already finished and
//! the buffers are back in hand. [`Started`] is that pair.
//!
//! # Why the synchronous case is visible rather than hidden
//!
//! It exists because of `FILE_SKIP_COMPLETION_PORT_ON_SUCCESS` (see
//! [`crate::Issued::Pending`] for what that mode changes). Without the mode,
//! every operation on an IOCP-associated endpoint produces a completion packet
//! -- even one that succeeded immediately -- so an adapter can always hand back
//! a claim-later token. With the mode, a synchronously-successful operation
//! produces no packet at all, and the token would name a completion that is
//! never coming.
//!
//! An adapter therefore cannot paper over the difference, because the two cases
//! do not merely differ in timing: they differ in *who owns the payload*. A
//! caller that ignored the distinction would either wait forever for a packet
//! that will not arrive, or drop a result that was already delivered. Making
//! both arms explicit costs a `match` and removes that whole class of mistake.
//!
//! A caller that never enables the mode will only ever observe
//! [`Started::Pending`], and can say so with [`Started::expect_pending`].

/// What became of an adapter submission that did not fail immediately.
///
/// This is [`crate::Submitted`] as an adapter reports it: the `Failed` arm is
/// folded into the enclosing `io::Result`'s `Err`, and the operation storage of
/// a synchronous completion is already reduced to the payload the matching
/// token's `claim` would have yielded, so the two arms report the same shape.
#[derive(Debug)]
pub enum Started<T, P> {
    /// The operation is in flight and a completion will arrive for it. Claim its
    /// result with the token, which also carries the operation's identity for
    /// cancellation and matching.
    Pending(T),
    /// The operation finished synchronously and no completion will arrive, so
    /// there is nothing to claim and its payload is returned directly.
    ///
    /// Only reachable on an endpoint in `FILE_SKIP_COMPLETION_PORT_ON_SUCCESS`
    /// mode; an endpoint left in the default mode always reports
    /// [`Started::Pending`], because the I/O Manager queues a packet even for an
    /// immediate success.
    Completed {
        /// The buffers the operation owned -- the same payload the token's
        /// `claim` yields on the pending path.
        payload: P,
        /// The byte count the native call reported.
        bytes_transferred: usize,
    },
}

impl<T, P> Started<T, P> {
    /// Whether a completion will arrive for this operation.
    #[must_use]
    pub fn is_pending(&self) -> bool {
        matches!(self, Started::Pending(_))
    }

    /// Whether the operation already finished with no completion to come.
    #[must_use]
    pub fn is_completed(&self) -> bool {
        matches!(self, Started::Completed { .. })
    }

    /// The token, if a completion will arrive.
    #[must_use]
    pub fn pending(self) -> Option<T> {
        match self {
            Started::Pending(token) => Some(token),
            Started::Completed { .. } => None,
        }
    }

    /// The payload and byte count, if the operation already finished.
    #[must_use]
    pub fn completed(self) -> Option<(P, usize)> {
        match self {
            Started::Completed {
                payload,
                bytes_transferred,
            } => Some((payload, bytes_transferred)),
            Started::Pending(_) => None,
        }
    }

    /// The token, panicking if the operation completed synchronously.
    ///
    /// For a caller that never puts its endpoints in
    /// `FILE_SKIP_COMPLETION_PORT_ON_SUCCESS` mode, where the synchronous arm is
    /// unreachable and matching on it is noise.
    ///
    /// # Panics
    ///
    /// Panics with `message` if the operation completed synchronously, which
    /// means the endpoint was in skip-on-success mode after all.
    #[must_use]
    #[track_caller]
    pub fn expect_pending(self, message: &str) -> T {
        match self {
            Started::Pending(token) => token,
            Started::Completed { .. } => panic!("{message}"),
        }
    }
}

#[cfg(test)]
mod tests;
