// Copyright (c) 2026 Mike Grier
//! Why a poll could not be started.

use std::{fmt, io};

/// Why [`Watcher::poll`](crate::Watcher::poll) or [`TailReader::poll`](crate::reader::TailReader::poll)
/// did not start a poll.
#[derive(Debug)]
#[non_exhaustive]
pub enum PollError {
    /// A poll is in flight. For a [`Watcher`](crate::Watcher), one is from the call that started it
    /// until its poll-done entry has been written, which includes one that is waiting for room in a
    /// full submission queue. For a [`TailReader`](crate::reader::TailReader), one is until its
    /// poll-done has been queued for the caller, which is after every read of the poll is delivered:
    /// the watcher having finished announcing is not enough, and a poll waiting for the caller to
    /// drain, or for chunks to be dropped, is still in flight.
    InFlight,
    /// The watcher has been shut down, and no poll starts again.
    Shutdown,
    /// The operating system refused to make what a poll runs on: a duplicate of the resume
    /// doorbell's event, the wait, or the thread-pool work item. The error carries the Win32 code.
    /// The watcher is as it was, and a later call tries again.
    Io(io::Error),
}

impl fmt::Display for PollError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InFlight => f.write_str("a poll is already in flight"),
            Self::Shutdown => f.write_str("the watcher has been shut down"),
            Self::Io(error) => write!(f, "making what the poll runs on failed: {error}"),
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
