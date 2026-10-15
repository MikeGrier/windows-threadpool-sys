// Copyright (c) Mike Grier.

//! The `IOCTL_STORAGE_QUERY_PROPERTY` call that the storage-property entries share (NR-2.3,
//! NR-2.4): the query structure it sends, and learning how big the reply is.
//!
//! One call, several entries: each property is its own entry with its own typed reply
//! ([`crate::storage_unique_id`]), because each reply is a different structure, and what they
//! share is this module, which is private apart from [`StoragePropertyError`].
//!
//! # The reply starts with its own size, and the caller asks again
//!
//! Every reply begins with a `STORAGE_DESCRIPTOR_HEADER`: a version, then the size in bytes of
//! the whole reply. A buffer of exactly the header's size is answered with the header alone,
//! which is how the size is learned; a buffer between that and the full size is answered with
//! `ERROR_MORE_DATA` and the header written; one smaller than the header is `ERROR_BAD_LENGTH`
//! (measured, below). The entry asks once with a buffer it expects to suffice, reads the size from
//! whichever of those comes back, and asks again at that size. The retry is bounded and ends in
//! [`StoragePropertyError::Unstable`], for the reason [`crate::final_path`]'s is.
//!
//! The query structure is sent in full: a shorter input is `ERROR_BAD_LENGTH` too.

use std::fmt;
use std::mem::offset_of;

use windows_sys::Win32::Foundation::{ERROR_MORE_DATA, HANDLE};
use windows_sys::Win32::System::Ioctl::{
    IOCTL_STORAGE_QUERY_PROPERTY, PropertyStandardQuery, STORAGE_DESCRIPTOR_HEADER,
    STORAGE_PROPERTY_ID, STORAGE_PROPERTY_QUERY,
};

use crate::device_control::{device_control, u32_at, zeroed_output};
use crate::outcome::{Outcome, Win32Error};

#[cfg(test)]
pub(crate) mod tests;

/// How many bytes the first attempt has room for.
///
/// A device-unique identifier on the disk measured was a hundred or so bytes, and a property
/// descriptor is usually less than a few hundred, so the common case costs one call; a larger
/// reply is asked for again at the size the device reports.
pub(crate) const FIRST_BYTES: usize = 512;

/// How many times the buffer is grown before the reply is called unstable.
pub(crate) const MAX_ATTEMPTS: usize = 4;

/// The size of `STORAGE_DESCRIPTOR_HEADER`, which every reply begins with.
pub(crate) const HEADER: usize = size_of::<STORAGE_DESCRIPTOR_HEADER>();

/// Why a storage property could not be read.
#[derive(Debug)]
#[non_exhaustive]
pub enum StoragePropertyError {
    /// Windows refused the call, with the raw code unaltered.
    ///
    /// `ERROR_NOT_ENOUGH_MEMORY` is also reported this way when the buffer for a size the device
    /// named could not be allocated, in which case the call was not made again.
    Win32(Win32Error),
    /// The call succeeded, or named a size, but the bytes do not hold what they say.
    ///
    /// Reported and not read, because a descriptor taken from a buffer that does not hold it
    /// would read as a property the device does not have.
    Malformed {
        /// What was wrong with the reply.
        what: &'static str,
    },
    /// The size kept changing, so the retry was abandoned.
    Unstable {
        /// How many attempts were made before giving up.
        attempts: usize,
    },
}

impl fmt::Display for StoragePropertyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Win32(error) => write!(f, "IOCTL_STORAGE_QUERY_PROPERTY: {error}"),
            Self::Malformed { what } => write!(f, "IOCTL_STORAGE_QUERY_PROPERTY: {what}"),
            Self::Unstable { attempts } => write!(
                f,
                "IOCTL_STORAGE_QUERY_PROPERTY: the reply size changed on each of {attempts} attempts"
            ),
        }
    }
}

impl std::error::Error for StoragePropertyError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Win32(error) => Some(error),
            Self::Malformed { .. } | Self::Unstable { .. } => None,
        }
    }
}

impl From<Win32Error> for StoragePropertyError {
    fn from(error: Win32Error) -> Self {
        Self::Win32(error)
    }
}

/// `STORAGE_PROPERTY_QUERY` for `property`, a standard query, written field by field so no byte
/// view of a struct is needed.
fn query_input(property: STORAGE_PROPERTY_ID) -> [u8; size_of::<STORAGE_PROPERTY_QUERY>()] {
    let mut input = [0_u8; size_of::<STORAGE_PROPERTY_QUERY>()];
    let mut put = |offset: usize, value: i32| {
        input[offset..offset + size_of::<i32>()].copy_from_slice(&value.to_ne_bytes());
    };
    put(offset_of!(STORAGE_PROPERTY_QUERY, PropertyId), property);
    put(
        offset_of!(STORAGE_PROPERTY_QUERY, QueryType),
        PropertyStandardQuery,
    );
    input
}

/// The reply to a standard query for `property` on `device`: exactly the bytes the reply's own
/// header says it has.
pub(crate) fn query_property(
    device: HANDLE,
    property: STORAGE_PROPERTY_ID,
    first: usize,
) -> Result<Vec<u8>, StoragePropertyError> {
    let input = query_input(property);
    query_via(first, |output| {
        device_control(device, IOCTL_STORAGE_QUERY_PROPERTY, &input, output)
    })
}

/// The reply `ask` gives, which fills the buffer it is lent and says how many bytes it wrote, or
/// reports `ERROR_MORE_DATA` with the header written when the buffer is too small for the reply.
///
/// Separate from the call so that a reply larger than the first buffer, which the disk measured
/// does not give, can be had from a test.
pub(crate) fn query_via(
    first: usize,
    mut ask: impl FnMut(&mut [u8]) -> Outcome<u32>,
) -> Result<Vec<u8>, StoragePropertyError> {
    let malformed = |what| StoragePropertyError::Malformed { what };
    let mut capacity = first.max(HEADER);

    for _ in 0..MAX_ATTEMPTS {
        let mut buffer = zeroed_output(capacity)?;

        let outcome = ask(&mut buffer);
        // The buffer is never smaller than the header, so the size is always there; the places
        // that handle its absence keep a damaged call an error and not a panic.
        let size = || {
            u32_at(&buffer, offset_of!(STORAGE_DESCRIPTOR_HEADER, Size))
                .and_then(|size| usize::try_from(size).ok())
        };

        match outcome {
            Ok(written) => {
                let written = usize::try_from(written)
                    .ok()
                    .filter(|written| *written <= buffer.len())
                    .ok_or(malformed(
                        "a call that wrote more than it was given room for",
                    ))?;
                if written < HEADER {
                    return Err(malformed("a reply shorter than its header"));
                }
                let size = size().ok_or(malformed("a reply with no size"))?;
                if size < HEADER {
                    return Err(malformed("a reply that says it is smaller than its header"));
                }
                if size <= written {
                    buffer.truncate(size);
                    return Ok(buffer);
                }
                if size <= buffer.len() {
                    return Err(malformed("a reply shorter than the size it names"));
                }
                // The header alone, or less than the reply: ask again at the size it names.
                capacity = size;
            }
            Err(error) if error.code() == ERROR_MORE_DATA => {
                let size =
                    size().ok_or(malformed("a reply that needs more room carries no size"))?;
                if size <= buffer.len() {
                    return Err(malformed(
                        "a reply that needs more room names a size already given",
                    ));
                }
                capacity = size;
            }
            Err(error) => return Err(error.into()),
        }
    }

    Err(StoragePropertyError::Unstable {
        attempts: MAX_ATTEMPTS,
    })
}
