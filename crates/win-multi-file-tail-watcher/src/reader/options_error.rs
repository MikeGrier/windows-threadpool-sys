// Copyright (c) 2026 Mike Grier
//! Why a size was refused.

use std::{fmt, io};

/// The rule a refused size broke.
///
/// Not an error by itself: [`OptionsError`] carries it as its `cause` and puts it in its message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Cause {
    /// The size is zero. For a ring queue the ring would accept it and choose a size of its own,
    /// which the caller did not ask for.
    Zero,
    /// The size is not a power of two. A ring would round it up, and a queue refuses it.
    NotAPowerOfTwo,
    /// The size is below the least the option can be.
    Below {
        /// The least size the option accepts, given the other options.
        least: u64,
    },
    /// The size is above the most the option can be.
    Above {
        /// The most the option accepts.
        most: u64,
    },
}

impl fmt::Display for Cause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Zero => f.write_str("it is zero"),
            Self::NotAPowerOfTwo => f.write_str("it is not a power of two"),
            Self::Below { least } => write!(f, "it is below the least allowed, {least}"),
            Self::Above { most } => write!(f, "it is above the most allowed, {most}"),
        }
    }
}

/// Why [`TailReaderOptions::validate`](crate::reader::TailReaderOptions::validate) refused a size.
///
/// Each refusal is a variant of its own, named for the option, and carries the value given and the
/// [`Cause`]. The first option that fails is the one reported, checked in the order the variants
/// are declared. The message names the option, the value and the cause, so it can be shown as it
/// is; [`cause`](Self::cause) gives the cause to a caller who wants to act on it.
#[derive(Debug)]
#[non_exhaustive]
pub enum OptionsError {
    /// `buffer_len` was refused.
    BufferLen {
        /// The size given.
        given: usize,
        /// The rule it broke.
        cause: Cause,
    },
    /// `buffer_count` was refused.
    BufferCount {
        /// The count given.
        given: usize,
        /// The rule it broke.
        cause: Cause,
    },
    /// `client_sq_capacity` was refused.
    ClientSqCapacity {
        /// The capacity given.
        given: usize,
        /// The rule it broke.
        cause: Cause,
    },
    /// `ring_sq_size` was refused.
    RingSqSize {
        /// The size given.
        given: u32,
        /// The rule it broke.
        cause: Cause,
    },
    /// `ring_cq_size` was refused.
    RingCqSize {
        /// The size given.
        given: u32,
        /// The `ring_sq_size` it was checked against, which sets the least it can be.
        ring_sq_size: u32,
        /// The rule it broke.
        cause: Cause,
    },
    /// The ring's own limits could not be read, so its two sizes could not be checked. The error
    /// is the operating system's and is this error's [`source`](std::error::Error::source).
    RingCapabilities(io::Error),
}

impl OptionsError {
    /// The rule the refused size broke, or `None` when the ring's limits could not be read.
    #[must_use]
    pub fn cause(&self) -> Option<Cause> {
        match self {
            Self::BufferLen { cause, .. }
            | Self::BufferCount { cause, .. }
            | Self::ClientSqCapacity { cause, .. }
            | Self::RingSqSize { cause, .. }
            | Self::RingCqSize { cause, .. } => Some(*cause),
            Self::RingCapabilities(_) => None,
        }
    }
}

impl fmt::Display for OptionsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BufferLen { given, cause } => {
                write!(f, "`buffer_len` {given} is refused: {cause}")
            }
            Self::BufferCount { given, cause } => {
                write!(f, "`buffer_count` {given} is refused: {cause}")
            }
            Self::ClientSqCapacity { given, cause } => {
                write!(f, "`client_sq_capacity` {given} is refused: {cause}")
            }
            Self::RingSqSize { given, cause } => {
                write!(f, "`ring_sq_size` {given} is refused: {cause}")
            }
            Self::RingCqSize {
                given,
                ring_sq_size,
                cause,
            } => write!(
                f,
                "`ring_cq_size` {given} is refused for a `ring_sq_size` of {ring_sq_size}: {cause}"
            ),
            Self::RingCapabilities(error) => {
                write!(f, "the ring's limits could not be read: {error}")
            }
        }
    }
}

impl std::error::Error for OptionsError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::RingCapabilities(error) => Some(error),
            Self::BufferLen { .. }
            | Self::BufferCount { .. }
            | Self::ClientSqCapacity { .. }
            | Self::RingSqSize { .. }
            | Self::RingCqSize { .. } => None,
        }
    }
}
