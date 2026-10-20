// Copyright (c) 2026 Mike Grier
//! Why a submission queue could not be created.

use std::{fmt, io};

use crate::SqCapacityError;
use crate::sq::QueueError;

/// Why a submission queue could not be made: the watcher's, by [`Watcher::new`](crate::Watcher::new),
/// or the reading layer's client queue, which reaches a caller as
/// [`TailReaderError::ClientSq`](crate::reader::TailReaderError::ClientSq). The messages name the
/// queue and not which of the two it was; the variant that holds it says.
///
/// Match it with a wildcard arm: a cause may be added without it being a breaking change.
#[derive(Debug)]
#[non_exhaustive]
pub enum CreateError {
    /// The queue cannot have the capacity asked for: it is not a power of two within the bounds the
    /// error carries. The same request is refused on every machine.
    Capacity(SqCapacityError),
    /// The capacity asked for is one the queue accepts, and the memory for its slots
    /// could not be allocated. The error carries the capacity and the bounds, which are what the
    /// queue accepts and not what this machine can hold; a smaller capacity may succeed. It has no
    /// source: the allocator's refusal carries no error value.
    Allocation(SqCapacityError),
    /// The operating system refused to create a doorbell. The error carries the Win32 code.
    Io(io::Error),
}

impl fmt::Display for CreateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Capacity(error) => write!(f, "creating the submission queue failed: {error}"),
            Self::Allocation(error) => write!(
                f,
                "creating the submission queue failed: SQ capacity {} is within the accepted range, {} to {}, \
                 but the memory for its slots could not be allocated",
                error.requested, error.min_valid, error.max_valid
            ),
            Self::Io(error) => write!(
                f,
                "creating the submission queue's doorbells failed: {error}"
            ),
        }
    }
}

impl std::error::Error for CreateError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Capacity(error) => Some(error),
            Self::Allocation(_) => None,
            Self::Io(error) => Some(error),
        }
    }
}

impl From<SqCapacityError> for CreateError {
    fn from(error: SqCapacityError) -> Self {
        Self::Capacity(error)
    }
}

impl From<QueueError> for CreateError {
    fn from(error: QueueError) -> Self {
        match error {
            QueueError::Capacity(error) => Self::Capacity(error),
            QueueError::Allocation(error) => Self::Allocation(error),
        }
    }
}

impl From<io::Error> for CreateError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}
