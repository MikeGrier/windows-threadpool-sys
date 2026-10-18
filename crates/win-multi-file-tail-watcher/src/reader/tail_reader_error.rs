// Copyright (c) 2026 Mike Grier
//! Why a [`TailReader`](super::TailReader) could not be made.

use std::collections::TryReserveError;
use std::{fmt, io};

use super::options_error::OptionsError;
use crate::CreateError;
use crate::pool::PoolError;
use crate::read_engine::EngineError;

/// Why [`TailReader::new`](super::TailReader::new) failed.
///
/// Each variant is one cause, and carries the error behind it as its
/// [`source`](std::error::Error::source), so a caller reads why without parsing text. A refused
/// size is [`Options`](Self::Options), found before anything is allocated; every other variant is
/// something a size that passed could still run into.
#[derive(Debug)]
#[non_exhaustive]
pub enum TailReaderError {
    /// A size in the [`TailReaderOptions`](super::TailReaderOptions) was refused. Nothing was
    /// allocated.
    Options(OptionsError),
    /// The watcher could not be made: the capacity given for its submission queue is not one it can
    /// have ([`CreateError::Capacity`]), is one it can have and the memory for it could not be
    /// allocated ([`CreateError::Allocation`]), or the operating system refused to create a
    /// doorbell.
    Watcher(CreateError),
    /// The client's queue could not be made. The options refuse the capacities it cannot have
    /// first, so this is reached for a doorbell the operating system refused to create, or for a
    /// capacity the queue accepts and the memory for it could not be allocated
    /// ([`CreateError::Allocation`]): the options accept any capacity the queue does, and the
    /// allocator answers here.
    ClientSq(CreateError),
    /// The pool's buffers could not be allocated: `buffer_len` times `buffer_count` bytes is more
    /// than the allocator would give. The options accept any count, and the allocator answers here.
    Allocation(TryReserveError),
    /// The I/O ring could not be made, or cannot read. On a Windows without I/O ring support this
    /// is the error that says so.
    Ring(io::Error),
    /// The operating system refused something else the layer needs: a doorbell for the buffer
    /// pool, a duplicate of an event, a thread-pool wait, or attaching the ring's completion event.
    /// The error carries the Win32 code.
    Io(io::Error),
}

impl TailReaderError {
    /// What a pool could not do.
    ///
    /// The pool's size and count were checked by the options with the very function the pool
    /// checks them with, so a refusal of either here would mean the options had passed what they
    /// refuse.
    pub(crate) fn from_pool(error: PoolError) -> Self {
        match error {
            PoolError::Allocation(error) => Self::Allocation(error),
            PoolError::Io(error) => Self::Io(error),
            PoolError::BufferLen(_) | PoolError::NoBuffers => {
                unreachable!("the options refused this size with the pool's own check")
            }
        }
    }

    /// What an engine could not do.
    pub(crate) fn from_engine(error: EngineError) -> Self {
        match error {
            EngineError::Sq(error) => Self::ClientSq(error),
            EngineError::Ring(error) => Self::Ring(error),
        }
    }
}

impl fmt::Display for TailReaderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Options(error) => write!(f, "a size was refused: {error}"),
            Self::Watcher(error) => write!(f, "the reader's watcher could not be made: {error}"),
            Self::ClientSq(error) => {
                write!(f, "the reader's client queue could not be made: {error}")
            }
            Self::Allocation(error) => {
                write!(f, "the reader's buffers could not be allocated: {error}")
            }
            Self::Ring(error) => write!(f, "the reader's I/O ring could not be made: {error}"),
            Self::Io(error) => write!(f, "the reader could not set itself up: {error}"),
        }
    }
}

impl std::error::Error for TailReaderError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Options(error) => Some(error),
            Self::Watcher(error) | Self::ClientSq(error) => Some(error),
            Self::Allocation(error) => Some(error),
            Self::Ring(error) | Self::Io(error) => Some(error),
        }
    }
}

impl From<OptionsError> for TailReaderError {
    fn from(error: OptionsError) -> Self {
        Self::Options(error)
    }
}
