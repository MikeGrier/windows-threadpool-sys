// Copyright (c) 2026 Mike Grier
//! Why a poll could not be started.

use std::{fmt, io};

/// Why [`Watcher::poll`](crate::Watcher::poll) did not start a poll.
#[derive(Debug)]
#[non_exhaustive]
pub enum PollError {
    /// A poll is in flight. One is from the call that started it until its poll-done entry has been
    /// written, which includes one that is waiting for room in a full submission queue.
    InFlight,
    /// The watcher has been shut down, and no poll starts again.
    Shutdown,
    /// The operating system refused to create the thread-pool work item. The error carries the
    /// Win32 code.
    Io(io::Error),
}

impl fmt::Display for PollError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InFlight => f.write_str("a poll is already in flight"),
            Self::Shutdown => f.write_str("the watcher has been shut down"),
            Self::Io(error) => write!(f, "creating the poll's thread-pool work failed: {error}"),
        }
    }
}

impl std::error::Error for PollError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InFlight | Self::Shutdown => None,
            Self::Io(error) => Some(error),
        }
    }
}

impl From<io::Error> for PollError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}
