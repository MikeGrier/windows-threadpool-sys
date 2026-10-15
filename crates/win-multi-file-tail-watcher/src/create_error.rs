// Copyright (c) 2026 Mike Grier
//! Why a watcher could not be created.

use std::{fmt, io};

use crate::SqCapacityError;

/// Why [`Watcher::new`](crate::Watcher::new) failed.
#[derive(Debug)]
pub enum CreateError {
    /// The submission queue cannot have the capacity asked for.
    Capacity(SqCapacityError),
    /// The operating system refused to create a doorbell. The error carries the Win32 code.
    Io(io::Error),
}

impl fmt::Display for CreateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Capacity(error) => write!(f, "creating the watcher failed: {error}"),
            Self::Io(error) => write!(f, "creating the watcher's doorbells failed: {error}"),
        }
    }
}

impl std::error::Error for CreateError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Capacity(error) => Some(error),
            Self::Io(error) => Some(error),
        }
    }
}

impl From<SqCapacityError> for CreateError {
    fn from(error: SqCapacityError) -> Self {
        Self::Capacity(error)
    }
}

impl From<io::Error> for CreateError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}
