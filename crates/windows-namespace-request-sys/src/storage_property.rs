// Copyright (c) Mike Grier.

//! The `IOCTL_STORAGE_QUERY_PROPERTY` call that the storage-property entries share (NR-2.3,
//! NR-2.4): the query structure it sends, and learning how big the reply is.
//!
//! One call, several entries: each property is its own entry with its own typed reply
//! ([`crate::storage_unique_id`]), because each reply is a different structure, and the generic
//! entry ([`crate::storage_query`]) is the same call with the property chosen by the caller and
//! the reply left as bytes. What they share is this module, which is private apart from
//! [`StoragePropertyError`].
//!
//! # The reply starts with its own size, and the caller asks again
//!
//! Every reply begins with a `STORAGE_DESCRIPTOR_HEADER`: a version, then the size in bytes of
//! the whole reply. A buffer of exactly the header's size is answered with the header alone,
//! which is how the size is learned. What a buffer *between* that and the full size, or one
//! *smaller* than the header, gets is the property's own, and two were measured to differ:
//!
//! | Property | Between the header and the reply | Smaller than the header |
//! |---|---|---|
//! | device unique identifier | `ERROR_MORE_DATA`, header written | `ERROR_BAD_LENGTH` |
//! | device NUMA node | success, header alone | `ERROR_INSUFFICIENT_BUFFER` |
//!
//! So the entry asks once with a buffer it expects to suffice, reads the size from the header of
//! whichever comes back (a failure that wrote the header, or a success that wrote only the
//! header), and asks again at that size, and it never asks with less than the header. The retry
//! is bounded and ends in [`StoragePropertyError::Unstable`], for the reason
//! [`crate::final_path`]'s is.
//!
//! The query structure is sent in full: a shorter input is `ERROR_BAD_LENGTH` too. Bytes after the
//! structure's own fixed fields are its additional parameters. Whether a property reads them is
//! the property's: on the disk measured the device property ignored any bytes, and the unique-id
//! property read the first, giving the usual reply for 0, a larger one for 1, and
//! `ERROR_INVALID_PARAMETER` for larger values. What that byte selects is not documented.
//!
//! # A query that returns no structure
//!
//! A query for whether a property exists (`PropertyExistsQuery`) returns no structure: success
//! with nothing written, whatever the size of the output, and an unknown property is a failure,
//! `ERROR_INVALID_FUNCTION` on the disk measured. It is a single call with no output, not the
//! learn-the-size loop.

use std::fmt;
use std::mem::offset_of;

use windows_sys::Win32::Foundation::{ERROR_MORE_DATA, HANDLE};
use windows_sys::Win32::System::Ioctl::{
    IOCTL_STORAGE_QUERY_PROPERTY, STORAGE_DESCRIPTOR_HEADER, STORAGE_PROPERTY_ID,
    STORAGE_PROPERTY_QUERY, STORAGE_QUERY_TYPE,
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

/// `STORAGE_PROPERTY_QUERY` for `property` and `query_type`, with `additional` after the
/// structure's fixed fields, written field by field so no byte view of a struct is needed.
///
/// The input is never shorter than the structure, which the device requires in full, and is as
/// long as the additional parameters need when they are longer. A buffer for caller-supplied
/// parameters is allocated fallibly, so a failure is an error the caller receives.
pub(crate) fn query_input(
    property: STORAGE_PROPERTY_ID,
    query_type: STORAGE_QUERY_TYPE,
    additional: &[u8],
) -> Result<Vec<u8>, StoragePropertyError> {
    let additional_at = offset_of!(STORAGE_PROPERTY_QUERY, AdditionalParameters);
    let length = additional_at
        .checked_add(additional.len())
        .ok_or(StoragePropertyError::Malformed {
            what: "additional parameters too long to hold",
        })?
        .max(size_of::<STORAGE_PROPERTY_QUERY>());

    let mut input = zeroed_output(length)?;
    let mut put = |offset: usize, value: i32| {
        input[offset..offset + size_of::<i32>()].copy_from_slice(&value.to_ne_bytes());
    };
    put(offset_of!(STORAGE_PROPERTY_QUERY, PropertyId), property);
    put(offset_of!(STORAGE_PROPERTY_QUERY, QueryType), query_type);
    input[additional_at..additional_at + additional.len()].copy_from_slice(additional);
    Ok(input)
}

/// The reply to the query `input` on `device`: exactly the bytes the reply's own header says it
/// has.
pub(crate) fn query_property(
    device: HANDLE,
    input: &[u8],
    first: usize,
) -> Result<Vec<u8>, StoragePropertyError> {
    query_via(first, |output| {
        device_control(device, IOCTL_STORAGE_QUERY_PROPERTY, input, output)
    })
}

/// Whether the query `input`, which asks if a property exists, succeeds on `device`: one call, with
/// no output, because the device returns no structure for it.
pub(crate) fn query_exists(device: HANDLE, input: &[u8]) -> Result<(), StoragePropertyError> {
    device_control(device, IOCTL_STORAGE_QUERY_PROPERTY, input, &mut [])?;
    Ok(())
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
