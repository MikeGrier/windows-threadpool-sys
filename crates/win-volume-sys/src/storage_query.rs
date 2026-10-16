// Copyright (c) 2026 Mike Grier
//! Any storage property: `IOCTL_STORAGE_QUERY_PROPERTY` with the property and the kind of query
//! chosen by the caller and the reply left as the bytes the device gave.
//!
//! The same call as the typed storage-property functions ([`storage_device_unique_id`],
//! [`storage_device_numa`]). It exists so a caller can reach a property this crate has no typed
//! function for, and so a property whose reply this crate does not interpret is still one call away.
//!
//! # What this does, and does not, do for the caller
//!
//! - The property is a `STORAGE_PROPERTY_ID` and the query a `STORAGE_QUERY_TYPE`, passed to the
//!   device as given. Neither is validated: a value the device does not know is the device's own
//!   failure, unaltered (`ERROR_INVALID_FUNCTION` for an unknown property and
//!   `ERROR_NOT_SUPPORTED` for an unknown query type or one the property does not take, on the
//!   disk measured).
//! - Additional parameters, for a property that takes some, follow the query structure's fixed
//!   fields, and are passed as given. Whether a property reads them is the property's, and a value
//!   it does not accept is the device's own failure; what they mean is not interpreted here.
//! - A standard query's reply starts with a `STORAGE_DESCRIPTOR_HEADER`, and the size is learned as
//!   the module for the call, `storage_property`, describes. The reply is **not interpreted**: the
//!   bytes are returned exactly as long as the header says. A query type whose reply does not start
//!   with that header would be refused as malformed, so what a caller gets from a type other than
//!   standard and exists is not established here: the mask query was refused by both properties
//!   tried.
//! - A query for whether a property exists returns no structure: the reply is empty and success
//!   means it exists.

use std::io;
use std::os::windows::io::AsHandle;

use windows_sys::Win32::System::Ioctl::{
    PropertyExistsQuery, PropertyMaskQuery, PropertyStandardQuery,
};

use crate::storage_property::{FIRST_BYTES, query_exists, query_input, query_property};
#[cfg(doc)]
use crate::{storage_device_numa, storage_device_unique_id};

#[cfg(test)]
mod tests;

/// A `STORAGE_PROPERTY_ID`: which property a query is for, as the raw value the device takes.
///
/// A newtype and not a bare `i32` so that a property and a query type cannot be swapped. The values
/// are `windows-sys`' (`StorageDeviceProperty` and the rest of that enumeration), which are not
/// reproduced here because the enumeration is the device's to extend.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct StoragePropertyId(i32);

impl StoragePropertyId {
    /// A property id, as the device takes it.
    #[must_use]
    pub const fn from_raw(raw: i32) -> Self {
        Self(raw)
    }

    /// The raw value.
    #[must_use]
    pub const fn raw(self) -> i32 {
        self.0
    }
}

/// A `STORAGE_QUERY_TYPE`: what a query asks about a property, as the raw value the device takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct StorageQueryType(i32);

impl StorageQueryType {
    /// `PropertyStandardQuery`: the property's descriptor.
    pub const STANDARD: Self = Self(PropertyStandardQuery);

    /// `PropertyExistsQuery`: whether the property exists. No structure is returned.
    pub const EXISTS: Self = Self(PropertyExistsQuery);

    /// `PropertyMaskQuery`: a mask of the descriptor's writable fields.
    pub const MASK: Self = Self(PropertyMaskQuery);

    /// A query type, as the device takes it.
    #[must_use]
    pub const fn from_raw(raw: i32) -> Self {
        Self(raw)
    }

    /// The raw value.
    #[must_use]
    pub const fn raw(self) -> i32 {
        self.0
    }
}

/// What `query_type` asks about `property` of the device `device` names, as the bytes the device
/// gave: exactly as long as the reply's own header says for a query that returns a structure, and
/// empty for one that returns none, where success means the property exists.
///
/// `device` is a handle to a disk, or another device that answers the control, and needs no access
/// to it beyond what opening it took. `additional` follows the query structure's fixed fields, for a
/// property that takes some; pass `&[]` for none.
///
/// # Example
///
/// Whether a disk has a property, and then the property's bytes. The property ids are
/// `windows-sys`':
///
/// ```no_run
/// use std::fs::OpenOptions;
/// use std::os::windows::fs::OpenOptionsExt;
///
/// use win_volume_sys::{query_storage_property, StoragePropertyId, StorageQueryType};
/// use windows_sys::Win32::System::Ioctl::StorageDeviceProperty;
///
/// let disk = OpenOptions::new()
///     .access_mode(0)
///     .share_mode(7)
///     .open(r"\\.\PhysicalDrive0")?;
/// let property = StoragePropertyId::from_raw(StorageDeviceProperty);
///
/// // Success means it exists; the reply has no structure.
/// let exists = query_storage_property(&disk, property, StorageQueryType::EXISTS, &[])?;
/// assert!(exists.is_empty());
///
/// let reply = query_storage_property(&disk, property, StorageQueryType::STANDARD, &[])?;
/// println!("{} bytes", reply.len());
/// # Ok::<(), std::io::Error>(())
/// ```
///
/// # Errors
///
/// The device's own error, with its code exactly: an unknown property, a query type the property
/// does not take, a handle that is not a disk. [`io::ErrorKind::InvalidData`] for a reply that does
/// not hold what its header says, which includes a reply with no header from a query type that was
/// expected to have one, and for a size that kept changing between the calls. An allocation that
/// cannot be made for a size the device named, or for additional parameters, is
/// `ERROR_NOT_ENOUGH_MEMORY`, and the call is not made again.
pub fn query_storage_property(
    device: &impl AsHandle,
    property: StoragePropertyId,
    query_type: StorageQueryType,
    additional: &[u8],
) -> io::Result<Vec<u8>> {
    let input = query_input(property.0, query_type.0, additional)?;

    if query_type == StorageQueryType::EXISTS {
        query_exists(device, &input)?;
        return Ok(Vec::new());
    }

    query_property(device, &input, FIRST_BYTES)
}
