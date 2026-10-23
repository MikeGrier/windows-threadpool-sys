// Copyright (c) 2026 Mike Grier
//! The backend beneath the core: what runs targets' I/O (DI-D-50 P10, DI-D-52).
//!
//! A backend is a ring: [`IoBackend::push`] queues a [`Submission`], [`IoBackend::submit`] is the
//! doorbell, [`IoBackend::pop`] takes a [`Completion`], and the backend's notification is raised
//! on every transition of its completions from none to some. A push only queues: what is queued
//! is issued by the doorbell, which the engine rings after every push it makes, once for a batch,
//! so that nothing waits for a completion to be issued (DI-7+.2). A backend may issue a submission
//! earlier, but never later than the doorbell. It has no threads of its own: who waits on the
//! notification and drains is the driver's delivery (DI-D-52).
//!
//! The submission is the core's shape, not the backend's: a write, a read or a commit naming a
//! target by its [`IoTargetKey`], which the backend maps to whatever it holds for that target.
//! Buffers go in owned and come back with the completion; a registered region is a backend
//! capability. A commit covers a whole target.
//!
//! **A refusal names its cause** (DI-D-52), so the caller takes the action that clears it --
//! submit, drain -- without probing for why. A backend that cannot tell says so with
//! [`RefusalCause::Unknown`], carrying what it was told.

use alloc::sync::Arc;
use core::fmt;
use std::io;

use crate::alloc_tags::{tag, tagged};
use crate::io_target::IoTarget;
use crate::types::{IoFault, IoTargetKey, WriteCaching};

#[cfg(test)]
mod tests;

/// The core's name for one submission, handed back on its completion. Unique among the
/// submissions an instance has in flight; what it encodes is the core's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SubmissionToken(pub u64);

/// A region of one of the backend's registered buffers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RegisteredRegion {
    /// Which registered buffer, by its position in the registration.
    pub index: u32,
    /// Where in that buffer the region starts.
    pub offset: u32,
    /// How many bytes it holds.
    pub length: u32,
}

/// What a write takes its bytes from, or a read puts them in.
#[derive(Debug)]
pub enum SubmissionData<B> {
    /// A buffer the submission owns, handed back with its completion.
    Owned(B),
    /// A region of a registered buffer.
    Registered(RegisteredRegion),
}

/// What a submission asks of its target.
#[derive(Debug)]
pub enum SubmissionOp<B> {
    /// Write the data at `offset`. It completes in full or with a fault, never short (DI-D-50).
    Write {
        /// Where in the target.
        offset: u64,
        /// The bytes.
        data: SubmissionData<B>,
        /// Whether the write may rest in a cache.
        caching: WriteCaching,
    },
    /// Read into the data from `offset`. It may complete short.
    Read {
        /// Where in the target.
        offset: u64,
        /// Where the bytes go; its length is the most the read transfers.
        data: SubmissionData<B>,
    },
    /// Commit every write to the target that completed before this was submitted (DI-D-50 P2).
    Commit,
}

/// One submission: what the core asks the backend to do.
#[derive(Debug)]
pub struct Submission<B> {
    /// The core's name for it.
    pub token: SubmissionToken,
    /// The target it addresses.
    pub target: IoTargetKey,
    /// What it asks.
    pub op: SubmissionOp<B>,
}

/// How a submission ended, as the backend reports it.
#[derive(Debug)]
pub struct Completion<B> {
    /// The submission's token.
    pub token: SubmissionToken,
    /// The bytes transferred -- all of a write's, and zero for a commit -- or the fault it ended
    /// with.
    pub result: Result<u32, IoFault>,
    /// The submission's owned buffer, handed back; `None` for a registered region or a commit.
    pub buffer: Option<B>,
}

/// Why a backend refused a submission (DI-D-52), so the caller can take the action that clears it
/// without probing. New causes may be added.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum RefusalCause {
    /// The submission queue is full: submitting what is queued makes room.
    SubmitNeeded,
    /// No more results fit in the completion queue: popping completions makes room.
    DrainNeeded,
    /// Both: submit what is queued, and pop completions.
    SubmitAndDrainNeeded,
    /// Another submission is under way; retry once it returns.
    Busy,
    /// No target is attached under the submission's key.
    UnknownTarget,
    /// The backend cannot perform what is asked at all -- a registered region on a backend with
    /// no registered buffers, for one. Retrying does not help.
    Unsupported,
    /// The submission is malformed for its target or the backend: an offset, length or buffer the
    /// target's geometry does not admit, or a region outside its registered buffer. Retrying does
    /// not help.
    Invalid,
    /// The target's access does not cover the operation: a write to a target that cannot write, or a
    /// read from one that cannot read (DI-D-66). Retrying does not help.
    AccessDenied,
    /// The backend can take no more submissions, ever; carries what it reported.
    Unusable(IoFault),
    /// The backend could not say why; carries what it reported.
    Unknown(IoFault),
}

impl RefusalCause {
    /// Whether submitting what is queued is part of clearing it.
    #[must_use]
    pub fn needs_submit(&self) -> bool {
        matches!(self, Self::SubmitNeeded | Self::SubmitAndDrainNeeded)
    }

    /// Whether popping completions is part of clearing it.
    #[must_use]
    pub fn needs_drain(&self) -> bool {
        matches!(self, Self::DrainNeeded | Self::SubmitAndDrainNeeded)
    }

    /// Whether the same submission may succeed if retried: after submitting or draining, as
    /// [`needs_submit`](Self::needs_submit) and [`needs_drain`](Self::needs_drain) say, or once
    /// another submission returns. An unknown cause may or may not clear; it answers `false`.
    #[must_use]
    pub fn is_retryable(&self) -> bool {
        self.needs_submit() || self.needs_drain() || matches!(self, Self::Busy)
    }
}

impl fmt::Display for RefusalCause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SubmitNeeded => f.write_str("the submission queue is full; submit"),
            Self::DrainNeeded => f.write_str("the completion queue is full; drain completions"),
            Self::SubmitAndDrainNeeded => {
                f.write_str("both queues are full; submit and drain completions")
            }
            Self::Busy => f.write_str("another submission is under way"),
            Self::UnknownTarget => f.write_str("no target is attached under that key"),
            Self::Unsupported => f.write_str("the backend cannot perform it"),
            Self::Invalid => f.write_str("the submission is malformed for its target"),
            Self::AccessDenied => f.write_str("the target's access does not cover the operation"),
            Self::Unusable(fault) => write!(f, "the backend is unusable: {}", fault.error),
            Self::Unknown(fault) => write!(f, "refused for a reason not known: {}", fault.error),
        }
    }
}

/// The cause is the error: [`Unusable`](RefusalCause::Unusable) and [`Unknown`](RefusalCause::Unknown)
/// name what the backend reported as their source.
impl core::error::Error for RefusalCause {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match self {
            Self::Unusable(fault) | Self::Unknown(fault) => Some(fault.error.as_ref()),
            _ => None,
        }
    }
}

/// The source of the fault that ends a submission of the core's own -- an operation its gate
/// released, or a commit -- when the backend refuses it for a cause that is not retryable
/// (DI-D-54). Its code is the backend's, where the cause carries one, and its error is the
/// [`RefusalCause`] itself, recovered by downcasting.
pub const REFUSED: &str = "refused";

/// The fault a submission refused for `cause` ends with: see [`REFUSED`].
pub(crate) fn refusal_fault(cause: RefusalCause) -> IoFault {
    let code = match &cause {
        RefusalCause::Unusable(fault) | RefusalCause::Unknown(fault) => fault.code,
        _ => None,
    };
    let kind = match cause {
        RefusalCause::Unsupported => io::ErrorKind::Unsupported,
        RefusalCause::Invalid => io::ErrorKind::InvalidInput,
        RefusalCause::UnknownTarget => io::ErrorKind::NotFound,
        RefusalCause::AccessDenied => io::ErrorKind::PermissionDenied,
        _ => io::ErrorKind::Other,
    };
    let error = tagged(tag::BACKEND_REFUSAL_FAULT_ERROR, || {
        io::Error::new(kind, cause)
    });
    IoFault {
        source: REFUSED,
        code,
        error: tagged(tag::BACKEND_REFUSAL_FAULT_SHARED, || Arc::new(error)),
    }
}

/// A refused submission: why, and the submission handed back, buffer and all.
#[derive(Debug)]
pub struct Refused<B> {
    /// Why.
    pub cause: RefusalCause,
    /// The submission, unchanged.
    pub submission: Submission<B>,
}

/// A target was attached under a key already in use: the target handed back.
#[derive(Debug)]
pub struct DuplicateTarget<T> {
    /// The key.
    pub key: IoTargetKey,
    /// The target, unattached.
    pub target: T,
}

/// A backend: what runs its targets' I/O (DI-D-50 P10, DI-D-52).
///
/// Every submission pushed and then submitted completes exactly once, through [`pop`](Self::pop);
/// one refused by [`push`](Self::push) never does. A failed [`submit`](Self::submit) leaves what
/// was queued queued, for the next one, and names its cause as a refused push does.
pub trait IoBackend {
    /// What it runs I/O for.
    type Target: IoTarget;
    /// The owned buffers its submissions carry.
    type Buffer;
    /// What it raises when its completions go from none to some.
    type Notification;

    /// Run I/O for `target` under `key`.
    ///
    /// # Errors
    ///
    /// [`DuplicateTarget`], handing the target back, if `key` is attached already.
    fn attach(
        &mut self,
        key: IoTargetKey,
        target: Self::Target,
    ) -> Result<(), DuplicateTarget<Self::Target>>;

    /// Stop running I/O for the target under `key`, handing it back; `None` if there is none. A
    /// submission for it queued but not yet submitted still completes exactly once, through
    /// [`pop`](Self::pop): with a fault, or, for a backend that cannot take a queued submission
    /// back, issued as it would have been, to the file it was pushed for. The core detaches a
    /// target only with no operation outstanding, so a driver does not meet either.
    fn detach(&mut self, key: IoTargetKey) -> Option<Self::Target>;

    /// How many registered buffers it has; a [`RegisteredRegion`] names one by its position.
    fn registered_buffers(&self) -> u32;

    /// Whether [`push`](Self::push) would refuse `op` on `target` for a cause that does not depend
    /// on its queues: no target under the key, an operation it cannot perform, or one malformed for
    /// its target. The core asks before it holds an operation for its gate, so a gated operation is
    /// refused at the consumer's push for what an ungated one would be (DI-D-54). A push this
    /// accepts may still be refused for a full queue, or by a backend that has become unusable.
    ///
    /// # Errors
    ///
    /// The [`RefusalCause`] `push` would give.
    fn check(
        &self,
        target: IoTargetKey,
        op: &SubmissionOp<Self::Buffer>,
    ) -> Result<(), RefusalCause>;

    /// Queue `submission`, to be issued no later than the next [`submit`](Self::submit). It is
    /// the backend's from here until it completes: only a refusal hands it back.
    ///
    /// # Errors
    ///
    /// [`Refused`], naming its cause and handing the submission back.
    fn push(&mut self, submission: Submission<Self::Buffer>) -> Result<(), Refused<Self::Buffer>>;

    /// Issue every queued submission.
    ///
    /// # Errors
    ///
    /// The [`RefusalCause`] of the failure -- drain completions, retry once another submission
    /// returns, or the backend's report -- and what was queued stays queued.
    fn submit(&mut self) -> Result<(), RefusalCause>;

    /// Take the oldest completion, if there is one.
    ///
    /// # Errors
    ///
    /// The fault the backend reported, if it could not read its completions.
    fn pop(&mut self) -> Result<Option<Completion<Self::Buffer>>, IoFault>;

    /// A handle on its notification.
    ///
    /// # Errors
    ///
    /// The fault the backend reported, if it could not make one.
    fn notification(&self) -> Result<Self::Notification, IoFault>;
}
