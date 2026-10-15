// Copyright (c) Mike Grier.

//! The generic `IOCTL_STORAGE_QUERY_PROPERTY` entry (D-22).
//!
//! The same call as the typed storage-property entries ([`crate::storage_unique_id`]), with the
//! property and the kind of query chosen by the caller and the reply left as the bytes the device
//! gave. It exists so a consumer can reach a property this crate has no typed entry for, and so
//! a property whose reply this crate does not interpret is still one call away.
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
//!   [`crate::storage_property`] describes. The reply is **not interpreted**: the bytes are
//!   returned exactly as long as the header says. A query type whose reply does not start with that
//!   header would be refused as malformed, so what a consumer gets from a type other than standard
//!   and exists is not established here: the mask query was refused by both properties tried.
//! - A query for whether a property exists returns no structure: the reply is empty and success
//!   means it exists.

use std::fmt;
use std::sync::Arc;

use windows_sys::Win32::System::Ioctl::{
    PropertyExistsQuery, PropertyMaskQuery, PropertyStandardQuery,
};

use crate::device_control::u32_at;
use crate::handle::{CapturedHandle, HandleCaptureError};
use crate::storage_property::{
    FIRST_BYTES, StoragePropertyError, query_exists, query_input, query_property,
};

#[cfg(test)]
mod tests;

/// Where the version and the size are in the header every standard reply starts with
/// (`STORAGE_DESCRIPTOR_HEADER`).
mod header {
    pub(super) const VERSION: usize = 0;
    pub(super) const SIZE: usize = 4;
}

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

/// What a storage property query returned: the bytes the device gave.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct StoragePropertyReply {
    bytes: Vec<u8>,
}

impl StoragePropertyReply {
    /// The reply, exactly as long as its header says. Empty for a query that returns no structure.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// The reply's bytes, by value.
    #[must_use]
    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }

    /// The descriptor's version from its header, or `None` if the reply has no structure.
    #[must_use]
    pub fn version(&self) -> Option<u32> {
        u32_at(&self.bytes, header::VERSION)
    }

    /// The descriptor's size from its header, or `None` if the reply has no structure.
    #[must_use]
    pub fn size(&self) -> Option<u32> {
        u32_at(&self.bytes, header::SIZE)
    }
}

impl fmt::Display for StoragePropertyReply {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.bytes.is_empty() {
            write!(f, "no structure")
        } else {
            write!(f, "{} bytes", self.bytes.len())
        }
    }
}

/// An owned, marshalable parameter set for `DeviceIoControl` with `IOCTL_STORAGE_QUERY_PROPERTY`,
/// for any property.
///
/// # Example
///
/// Whether a disk has a property, and then the property's bytes. The property ids are
/// `windows-sys`':
///
/// ```no_run
/// use std::fs::OpenOptions;
/// use std::os::windows::fs::OpenOptionsExt;
/// use std::os::windows::io::AsHandle;
///
/// use windows_namespace_request_sys::storage_query::{
///     QueryStorageProperty, StoragePropertyId, StorageQueryType,
/// };
/// use windows_namespace_request_sys::CapturedHandle;
/// use windows_sys::Win32::System::Ioctl::StorageDeviceProperty;
///
/// let disk = OpenOptions::new()
///     .access_mode(0)
///     .share_mode(7)
///     .open(r"\\.\PhysicalDrive0")?;
/// let handle = CapturedHandle::capture(disk.as_handle())?;
/// let property = StoragePropertyId::from_raw(StorageDeviceProperty);
///
/// // Success means it exists; the reply has no structure.
/// let exists = QueryStorageProperty::new(handle.try_clone()?, property)
///     .with_query_type(StorageQueryType::EXISTS)
///     .perform()?;
/// assert!(exists.as_bytes().is_empty());
///
/// let reply = QueryStorageProperty::new(handle, property).perform()?;
/// println!("{} bytes, version {:?}", reply.as_bytes().len(), reply.version());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug)]
#[must_use = "an unperformed request queries nothing"]
pub struct QueryStorageProperty {
    handle: CapturedHandle,
    property: StoragePropertyId,
    query_type: StorageQueryType,
    additional: Arc<[u8]>,
}

impl QueryStorageProperty {
    /// Begins a standard query for `property` against `handle`, which names a disk or another
    /// device that answers the control.
    pub fn new(handle: CapturedHandle, property: StoragePropertyId) -> Self {
        Self {
            handle,
            property,
            query_type: StorageQueryType::STANDARD,
            additional: Arc::from([]),
        }
    }

    /// Sets what the query asks.
    pub fn with_query_type(mut self, query_type: StorageQueryType) -> Self {
        self.query_type = query_type;
        self
    }

    /// Sets the additional parameters that follow the query structure's fixed fields, for a
    /// property that takes some. They are shared, not copied, when the request is copied.
    pub fn with_additional_parameters(mut self, additional: impl Into<Arc<[u8]>>) -> Self {
        self.additional = additional.into();
        self
    }

    /// The owned duplicate of the handle being queried.
    pub fn handle(&self) -> &CapturedHandle {
        &self.handle
    }

    /// The property being queried.
    #[must_use]
    pub fn property(&self) -> StoragePropertyId {
        self.property
    }

    /// What the query asks.
    #[must_use]
    pub fn query_type(&self) -> StorageQueryType {
        self.query_type
    }

    /// The additional parameters, empty if none were set.
    #[must_use]
    pub fn additional_parameters(&self) -> &[u8] {
        &self.additional
    }

    /// Copies the request, duplicating the handle.
    ///
    /// # Errors
    ///
    /// Returns the handle-capture failure when the handle cannot be duplicated.
    pub fn try_clone(&self) -> Result<Self, HandleCaptureError> {
        Ok(Self {
            handle: self.handle.try_clone()?,
            property: self.property,
            query_type: self.query_type,
            additional: Arc::clone(&self.additional),
        })
    }

    /// The query structure this request sends, with its additional parameters.
    fn input(&self) -> Result<Vec<u8>, StoragePropertyError> {
        query_input(self.property.0, self.query_type.0, &self.additional)
    }

    /// Performs the query on the calling thread. For a query that returns a structure the size is
    /// learned from the reply's header, asking again if the first buffer was too small.
    ///
    /// # Errors
    ///
    /// [`StoragePropertyError::Win32`] with the raw code, unaltered;
    /// [`StoragePropertyError::Malformed`] for a reply that does not hold what it says, which
    /// includes a reply with no header from a query type that was expected to have one;
    /// [`StoragePropertyError::Unstable`] if the size kept changing.
    pub fn perform(&self) -> Result<StoragePropertyReply, StoragePropertyError> {
        let input = self.input()?;

        if self.query_type == StorageQueryType::EXISTS {
            query_exists(self.handle.raw(), &input)?;
            return Ok(StoragePropertyReply { bytes: Vec::new() });
        }

        let bytes = query_property(self.handle.raw(), &input, FIRST_BYTES)?;
        Ok(StoragePropertyReply { bytes })
    }
}

impl crate::request::Request for QueryStorageProperty {
    type Error = StoragePropertyError;
    type Output = StoragePropertyReply;

    fn perform(&self) -> Result<StoragePropertyReply, StoragePropertyError> {
        Self::perform(self)
    }
}
