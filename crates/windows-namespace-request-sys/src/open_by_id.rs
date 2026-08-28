// Copyright (c) Mike Grier.

//! The `OpenFileById` entry.
//!
//! Entry 2 of the audited catalogue, and a **second open primitive** rather
//! than a variant of [`crate::open::OpenFile`]. It takes a volume-hint handle
//! and a file identifier instead of a path, and it has no creation disposition
//! at all -- it can only open something that already exists. One entry per
//! Win32 call means it is its own entry.
//!
//! # Why a consumer reaches for this
//!
//! An identifier names a filesystem *object*, where a path names a location.
//! Reopening by id is structurally incapable of landing on a different object
//! than the one the id already named, while a fresh open against the original
//! path cannot tell a recreated directory from the one a consumer started on.
//! That difference is the whole reason the entry exists.
//!
//! # The volume hint is an input handle, and is owned
//!
//! This is the first entry to take a handle as an *input*, so it is the first
//! consumer of [`CapturedHandle`]. The hint only needs to name some still-open
//! handle on the same volume as the identifier -- it is never itself the object
//! being reopened, so it stays valid even once that object is gone. The request
//! owns a duplicate of it, so the request cannot be left naming a hint its
//! originator has closed.

use std::ffi::c_void;
use std::os::windows::io::{FromRawHandle, OwnedHandle};
use std::ptr;

use windows_sys::Win32::Storage::FileSystem::{
    ExtendedFileIdType, FILE_FLAGS_AND_ATTRIBUTES, FILE_ID_128, FILE_ID_DESCRIPTOR,
    FILE_ID_DESCRIPTOR_0, FILE_ID_TYPE, FILE_SHARE_MODE, FileIdType, ObjectIdType, OpenFileById,
};
use windows_sys::core::GUID;

use crate::handle::{CapturedHandle, HandleCaptureError};
use crate::outcome::{Outcome, perform_handle};
use crate::security::SecurityAttributes;

/// Which kind of identifier names the object to open.
///
/// Win32 spells this as a tagged union whose tag and payload must be kept in
/// step by hand; here the tag is implied by the variant, so the two cannot
/// disagree.
///
/// All three forms are supported. Only [`FileId`](Self::FileId) appears in the
/// audited consumers, but an entry that could express one of its own call's
/// three identifier kinds would be a narrowed `OpenFileById`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FileIdentifier {
    /// A 64-bit file reference number, as reported by `FileIdInfo`'s
    /// predecessor and by `BY_HANDLE_FILE_INFORMATION`. This is the form every
    /// audited consumer uses.
    FileId(u64),
    /// A volume-scoped object identifier.
    ///
    /// Taken as a `u128` rather than a `GUID` deliberately. Win32's `GUID` is a
    /// dependency's type that implements neither equality nor `Debug`, and a
    /// public surface should not be shaped by whichever binding crate this
    /// happens to build against. The conversion to `GUID` happens at the FFI
    /// boundary, where it belongs.
    ObjectId(u128),
    /// A 128-bit file reference number, as reported by `FileIdInfo` on ReFS,
    /// where 64 bits is not enough to name a file.
    ExtendedFileId([u8; 16]),
}

impl FileIdentifier {
    /// The `FILE_ID_TYPE` tag Win32 pairs with this identifier.
    #[must_use]
    pub fn id_type(self) -> FILE_ID_TYPE {
        match self {
            Self::FileId(_) => FileIdType,
            Self::ObjectId(_) => ObjectIdType,
            Self::ExtendedFileId(_) => ExtendedFileIdType,
        }
    }

    /// Builds the Win32 descriptor, with its tag and payload necessarily in
    /// step.
    fn to_descriptor(self) -> FILE_ID_DESCRIPTOR {
        let anonymous = match self {
            Self::FileId(id) => FILE_ID_DESCRIPTOR_0 {
                FileId: id.cast_signed(),
            },
            Self::ObjectId(id) => FILE_ID_DESCRIPTOR_0 {
                ObjectId: GUID::from_u128(id),
            },
            Self::ExtendedFileId(id) => FILE_ID_DESCRIPTOR_0 {
                ExtendedFileId: FILE_ID_128 { Identifier: id },
            },
        };

        FILE_ID_DESCRIPTOR {
            dwSize: u32::try_from(size_of::<FILE_ID_DESCRIPTOR>())
                .expect("this fixed, small struct's size always fits a u32"),
            Type: self.id_type(),
            Anonymous: anonymous,
        }
    }
}

/// An owned, marshalable parameter set for `OpenFileById`.
///
/// # Example
///
/// ```
/// use std::fs;
/// use std::os::windows::io::AsHandle;
///
/// use windows_namespace_request_sys::open_by_id::{FileIdentifier, OpenFileByIdentifier};
/// use windows_namespace_request_sys::CapturedHandle;
/// use windows_sys::Win32::Storage::FileSystem::{
///     FILE_GENERIC_READ, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
/// };
///
/// let path = std::env::temp_dir().join(format!("wnrs-byid-{}.tmp", std::process::id()));
/// fs::write(&path, b"example")?;
/// let file = fs::File::open(&path)?;
///
/// // Any still-open handle on the same volume will do as the hint; it is never
/// // the object being reopened.
/// let hint = CapturedHandle::capture(file.as_handle())?;
/// let request = OpenFileByIdentifier::new(hint, FileIdentifier::FileId(0))
///     .with_desired_access(FILE_GENERIC_READ)
///     .with_share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE);
///
/// assert_eq!(request.identifier(), FileIdentifier::FileId(0));
/// # drop(file);
/// # fs::remove_file(&path)?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug)]
#[must_use = "an unperformed request opens nothing"]
pub struct OpenFileByIdentifier {
    volume_hint: CapturedHandle,
    identifier: FileIdentifier,
    desired_access: u32,
    share_mode: FILE_SHARE_MODE,
    security: Option<SecurityAttributes>,
    flags_and_attributes: FILE_FLAGS_AND_ATTRIBUTES,
}

impl OpenFileByIdentifier {
    /// Begins a request to reopen `identifier`, using `volume_hint` to name the
    /// volume it lives on.
    ///
    /// As with [`crate::open::OpenFile`], the remaining parameters start at
    /// "the caller said nothing" and are set explicitly. There is no creation
    /// disposition to set: `OpenFileById` has none, which is one of the reasons
    /// this is a separate entry rather than a variant.
    pub fn new(volume_hint: CapturedHandle, identifier: FileIdentifier) -> Self {
        Self {
            volume_hint,
            identifier,
            desired_access: 0,
            share_mode: 0,
            security: None,
            flags_and_attributes: 0,
        }
    }

    /// Sets `dwDesiredAccess`.
    pub fn with_desired_access(mut self, desired_access: u32) -> Self {
        self.desired_access = desired_access;
        self
    }

    /// Sets `dwShareMode`.
    pub fn with_share_mode(mut self, share_mode: FILE_SHARE_MODE) -> Self {
        self.share_mode = share_mode;
        self
    }

    /// Sets `lpSecurityAttributes` from an already-captured value.
    pub fn with_security(mut self, security: Option<SecurityAttributes>) -> Self {
        self.security = security;
        self
    }

    /// Sets `dwFlagsAndAttributes`, carried verbatim as in
    /// [`crate::open::OpenFile`].
    pub fn with_flags_and_attributes(
        mut self,
        flags_and_attributes: FILE_FLAGS_AND_ATTRIBUTES,
    ) -> Self {
        self.flags_and_attributes = flags_and_attributes;
        self
    }

    /// The owned duplicate of the volume-hint handle.
    pub fn volume_hint(&self) -> &CapturedHandle {
        &self.volume_hint
    }

    /// The identifier this request will reopen.
    #[must_use]
    pub fn identifier(&self) -> FileIdentifier {
        self.identifier
    }

    /// The requested access mask.
    #[must_use]
    pub fn desired_access(&self) -> u32 {
        self.desired_access
    }

    /// The requested share mode.
    #[must_use]
    pub fn share_mode(&self) -> FILE_SHARE_MODE {
        self.share_mode
    }

    /// The captured security attributes, if any were supplied.
    #[must_use]
    pub fn security(&self) -> Option<&SecurityAttributes> {
        self.security.as_ref()
    }

    /// The requested flags and attributes.
    #[must_use]
    pub fn flags_and_attributes(&self) -> FILE_FLAGS_AND_ATTRIBUTES {
        self.flags_and_attributes
    }

    /// Copies the request, duplicating the volume-hint handle.
    ///
    /// Not `Clone`, for the same reason [`crate::open::OpenFile::try_clone`] is
    /// not: this request always owns a handle, and duplicating one is fallible.
    ///
    /// # Errors
    ///
    /// Returns the handle-capture failure when the hint cannot be duplicated.
    pub fn try_clone(&self) -> Result<Self, HandleCaptureError> {
        Ok(Self {
            volume_hint: self.volume_hint.try_clone()?,
            identifier: self.identifier,
            desired_access: self.desired_access,
            share_mode: self.share_mode,
            security: self.security.clone(),
            flags_and_attributes: self.flags_and_attributes,
        })
    }

    /// Performs the open on the calling thread.
    ///
    /// The handle comes back plain and unassociated, as with every other
    /// handle-producing entry.
    ///
    /// # Errors
    ///
    /// Returns the raw Win32 code, unaltered. `ERROR_INVALID_PARAMETER` here
    /// most often means the identified object no longer exists, but nothing in
    /// this crate infers that on the caller's behalf.
    pub fn perform(&self) -> Outcome<OwnedHandle> {
        let descriptor = self.identifier.to_descriptor();
        let attributes = self.security.as_ref().map(SecurityAttributes::to_raw);
        let attributes_ptr = attributes.as_ref().map_or(ptr::null(), ptr::from_ref);

        let raw = perform_handle(|| {
            // SAFETY: the volume hint is a duplicate this request owns and
            // keeps open across the call; the descriptor is fully initialised
            // with its tag and payload in step and is only read; the security
            // attributes, when present, point at a self-relative descriptor
            // this request owns.
            unsafe {
                OpenFileById(
                    self.volume_hint.raw(),
                    &raw const descriptor,
                    self.desired_access,
                    self.share_mode,
                    attributes_ptr,
                    self.flags_and_attributes,
                )
            }
        })?;

        // SAFETY: a successful OpenFileById returns a handle this process owns
        // exclusively and must release with CloseHandle, which OwnedHandle
        // does.
        Ok(unsafe { OwnedHandle::from_raw_handle(raw.cast::<c_void>()) })
    }
}

#[cfg(test)]
mod tests;
