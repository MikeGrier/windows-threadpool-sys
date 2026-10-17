// Copyright (c) 2026 Mike Grier
//! Why opening a file at a starting offset failed.

use std::{fmt, io};

/// Why [`HeldFile::open_at`](crate::HeldFile::open_at) failed.
///
/// The two are different kinds of failure: [`Io`](Self::Io) is the operating system refusing, and
/// [`OffsetBeyondLength`](Self::OffsetBeyondLength) is a caller whose idea of the file is ahead of
/// the file itself.
#[derive(Debug)]
pub enum OpenError {
    /// The open, or the read of the length that follows it, failed. The error carries the Win32
    /// code, read with [`io::Error::raw_os_error`]; a path containing a NUL has none.
    Io(io::Error),
    /// The starting offset is past the end of the file as it stood at open. An offset equal to the
    /// length is not past it.
    OffsetBeyondLength {
        /// The starting offset the caller asked for.
        offset: u64,
        /// The file's length when it was opened.
        len: u64,
    },
}

impl fmt::Display for OpenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "opening the file failed: {error}"),
            Self::OffsetBeyondLength { offset, len } => write!(
                f,
                "starting offset {offset} is beyond the file's length {len}"
            ),
        }
    }
}

impl std::error::Error for OpenError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::OffsetBeyondLength { .. } => None,
        }
    }
}

impl From<io::Error> for OpenError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}
