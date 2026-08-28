// Copyright (c) Mike Grier.

//! The `CreateFileW` entry.
//!
//! Entry 1 of the audited catalogue, and the only one all three audited
//! consumers use. It captures the complete parameter set on the calling thread
//! and performs the open faithfully wherever it is executed.
//!
//! # The overlapped split is a field, not a policy
//!
//! Two of the audited consumers open without `FILE_FLAG_OVERLAPPED` and one
//! opens with it, because the watcher's handle is destined for a completion
//! port and the other two are not. That difference belongs to the caller: this
//! entry carries whatever flags it was given and never adds, removes, or
//! second-guesses one. Deciding it here would be the delivery-model choice the
//! crate refuses to make -- an opened handle comes back plain and unassociated
//! either way, and associating it is a later layer's call.
//!
//! # Nothing is defaulted on the caller's behalf
//!
//! `FILE_FLAG_BACKUP_SEMANTICS` is mandatory to open a *directory* at all, and
//! every audited consumer passes it. It is still not implied here. An entry
//! that quietly added a flag would be deciding what the caller meant, and the
//! same field is what a caller opening a plain file must be able to leave out.

use std::ffi::c_void;
use std::os::windows::io::{FromRawHandle, OwnedHandle};
use std::ptr;

use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_CREATION_DISPOSITION, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_MODE,
};

use crate::handle::{CapturedHandle, HandleCaptureError};
use crate::outcome::{Outcome, perform_handle};
use crate::path::PreparedPath;
use crate::security::SecurityAttributes;

/// An owned, marshalable parameter set for `CreateFileW`.
///
/// Every parameter of the underlying call is expressible, including the two no
/// audited consumer uses. An entry that could not express two of its own call's
/// parameters would be a *narrowed* `CreateFileW`, and narrowing a platform
/// entry to fit the consumers currently in view is precisely the anti-pattern
/// this workspace's platform-integrity rule names.
///
/// # Example
///
/// ```
/// use std::fs;
///
/// use windows_namespace_request_sys::open::OpenFile;
/// use windows_namespace_request_sys::prepare;
/// use wtf_string::Wtf16String;
/// use windows_sys::Win32::Storage::FileSystem::{
///     FILE_GENERIC_READ, FILE_SHARE_READ, OPEN_EXISTING,
/// };
///
/// let path = std::env::temp_dir().join(format!("wnrs-open-{}.tmp", std::process::id()));
/// fs::write(&path, b"example")?;
///
/// // Built on this thread, where the current directory still means what the
/// // caller thinks it means.
/// let text = path.to_str().expect("a temporary path is valid UTF-8");
/// let request = OpenFile::new(prepare(&Wtf16String::from(text))?)
///     .with_desired_access(FILE_GENERIC_READ)
///     .with_share_mode(FILE_SHARE_READ)
///     .with_creation_disposition(OPEN_EXISTING);
///
/// // Performed here, but it would behave identically on any other thread.
/// let opened = fs::File::from(request.perform()?);
/// assert_eq!(opened.metadata()?.len(), b"example".len() as u64);
/// # drop(opened);
/// # fs::remove_file(&path)?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug)]
#[must_use = "an unperformed request opens nothing"]
pub struct OpenFile {
    path: PreparedPath,
    desired_access: u32,
    share_mode: FILE_SHARE_MODE,
    security: Option<SecurityAttributes>,
    creation_disposition: FILE_CREATION_DISPOSITION,
    flags_and_attributes: FILE_FLAGS_AND_ATTRIBUTES,
    template: Option<CapturedHandle>,
}

impl OpenFile {
    /// Begins a request against `path`.
    ///
    /// Every other parameter starts at the value that means "the caller said
    /// nothing": no access, no sharing, no security attributes, a zero creation
    /// disposition, no flags, and no template. They are set explicitly rather
    /// than defaulted to a plausible-looking open, because a plausible default
    /// is exactly what a caller cannot see they got.
    pub fn new(path: PreparedPath) -> Self {
        Self {
            path,
            desired_access: 0,
            share_mode: 0,
            security: None,
            creation_disposition: 0,
            flags_and_attributes: 0,
            template: None,
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
    ///
    /// Passing `None` means a null argument: default security and a
    /// non-inheritable handle. That is a different outcome from attributes
    /// carrying a null descriptor, which is why the distinction survives into
    /// this type rather than being flattened here.
    pub fn with_security(mut self, security: Option<SecurityAttributes>) -> Self {
        self.security = security;
        self
    }

    /// Sets `dwCreationDisposition`.
    pub fn with_creation_disposition(
        mut self,
        creation_disposition: FILE_CREATION_DISPOSITION,
    ) -> Self {
        self.creation_disposition = creation_disposition;
        self
    }

    /// Sets `dwFlagsAndAttributes`.
    ///
    /// Carried verbatim, including `FILE_FLAG_OVERLAPPED`. Whether the opened
    /// handle is destined for a completion port is the caller's to state and
    /// this crate's to leave alone.
    pub fn with_flags_and_attributes(
        mut self,
        flags_and_attributes: FILE_FLAGS_AND_ATTRIBUTES,
    ) -> Self {
        self.flags_and_attributes = flags_and_attributes;
        self
    }

    /// Sets `hTemplateFile` from an already-captured handle.
    ///
    /// The request owns a duplicate, so it cannot be left naming a template the
    /// caller has since closed.
    pub fn with_template(mut self, template: Option<CapturedHandle>) -> Self {
        self.template = template;
        self
    }

    /// The prepared path this request will open.
    #[must_use]
    pub fn path(&self) -> &PreparedPath {
        &self.path
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

    /// The requested creation disposition.
    #[must_use]
    pub fn creation_disposition(&self) -> FILE_CREATION_DISPOSITION {
        self.creation_disposition
    }

    /// The requested flags and attributes.
    #[must_use]
    pub fn flags_and_attributes(&self) -> FILE_FLAGS_AND_ATTRIBUTES {
        self.flags_and_attributes
    }

    /// The captured template handle, if one was supplied.
    #[must_use]
    pub fn template(&self) -> Option<&CapturedHandle> {
        self.template.as_ref()
    }

    /// Copies the request, duplicating the template handle if it has one.
    ///
    /// This is not `Clone` because a request may own a handle, and duplicating
    /// a handle is fallible. The type inherits that from
    /// [`CapturedHandle::try_clone`] rather than hiding it behind an infallible
    /// signature that would have to panic.
    ///
    /// # Errors
    ///
    /// Returns the handle-capture failure when the template cannot be
    /// duplicated. A request with no template cannot fail.
    pub fn try_clone(&self) -> Result<Self, HandleCaptureError> {
        let template = self
            .template
            .as_ref()
            .map(CapturedHandle::try_clone)
            .transpose()?;

        Ok(Self {
            path: self.path.clone(),
            desired_access: self.desired_access,
            share_mode: self.share_mode,
            security: self.security.clone(),
            creation_disposition: self.creation_disposition,
            flags_and_attributes: self.flags_and_attributes,
            template,
        })
    }

    /// Performs the open on the calling thread.
    ///
    /// The handle comes back **plain and unassociated**: nothing here binds it
    /// to a completion port, because doing so irreversibly forecloses `IoRing`
    /// use of it and that choice belongs to a layer that knows the handle's
    /// destination.
    ///
    /// # Errors
    ///
    /// Returns the raw Win32 code, unaltered and snapshotted before any cleanup
    /// can overwrite it. `ERROR_FILE_NOT_FOUND` here means a missing path and
    /// nothing else is inferred from it.
    pub fn perform(&self) -> Outcome<OwnedHandle> {
        let attributes = self.security.as_ref().map(SecurityAttributes::to_raw);
        let attributes_ptr = attributes.as_ref().map_or(ptr::null(), ptr::from_ref);
        let template = self
            .template
            .as_ref()
            .map_or(ptr::null_mut(), |handle| handle.raw());

        let raw = perform_handle(|| {
            // SAFETY: the path is NUL-terminated and outlives the call; the
            // security attributes, when present, are a live struct pointing at
            // a self-relative descriptor this request owns; the template, when
            // present, is a duplicate this request owns. Every other argument
            // is a plain value.
            unsafe {
                CreateFileW(
                    self.path.as_wtf16_terminated(),
                    self.desired_access,
                    self.share_mode,
                    attributes_ptr,
                    self.creation_disposition,
                    self.flags_and_attributes,
                    template,
                )
            }
        })?;

        // SAFETY: a successful CreateFileW returns a handle this process owns
        // exclusively and must release with CloseHandle, which OwnedHandle
        // does.
        Ok(unsafe { OwnedHandle::from_raw_handle(raw.cast::<c_void>()) })
    }
}

impl crate::request::Request for OpenFile {
    type Output = OwnedHandle;

    fn perform(&self) -> Outcome<OwnedHandle> {
        Self::perform(self)
    }
}

#[cfg(test)]
mod tests;
