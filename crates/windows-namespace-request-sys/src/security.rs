// Copyright (c) Mike Grier.

//! Owned capture of a caller's `lpSecurityAttributes`.
//!
//! A `SECURITY_ATTRIBUTES` is not a value. It points at a security descriptor,
//! and that descriptor may itself be **absolute**: a structure of raw pointers
//! to an owner SID, a group SID, a DACL and a SACL that are quite possibly on
//! the caller's stack. Carrying one to another thread by copying the struct
//! would carry a set of dangling pointers.
//!
//! Capture therefore normalises to the **self-relative** form, in which every
//! part lives at an offset inside one contiguous blob, and owns that blob.

use std::ffi::c_void;
use std::fmt;
use std::io;
use std::ptr;

use windows_sys::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, FALSE, TRUE};
use windows_sys::Win32::Security::{
    ACL, GetSecurityDescriptorControl, GetSecurityDescriptorDacl, GetSecurityDescriptorLength,
    GetSecurityDescriptorSacl, IsValidSecurityDescriptor, MakeSelfRelativeSD, PSECURITY_DESCRIPTOR,
    SE_SELF_RELATIVE, SECURITY_ATTRIBUTES, SECURITY_DESCRIPTOR_CONTROL,
};

use crate::buffer::AlignedBuffer;

/// A self-relative security descriptor must be DWORD-aligned.
///
/// Windows states this as a requirement on the buffer a self-relative
/// descriptor lives in, not as a property of any one field, which is why it is
/// enforced on the whole blob.
const SELF_RELATIVE_ALIGNMENT: usize = align_of::<u32>();

/// Why a caller's security attributes could not be captured.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum SecurityCaptureFailure {
    /// Windows rejected the descriptor as malformed.
    ///
    /// Reported at construction, on the calling thread, rather than left for
    /// the eventual call to fail with it on a worker.
    InvalidDescriptor,
    /// Windows could not report the descriptor's control flags, so whether it
    /// was absolute or self-relative could not be established.
    ReadControl,
    /// Windows could not size the self-relative form of an absolute
    /// descriptor.
    SizeSelfRelative,
    /// Windows could not convert an absolute descriptor to self-relative form.
    MakeSelfRelative,
    /// The captured copy did not validate, which would mean the conversion
    /// produced something Windows will not accept later.
    InvalidCopy,
    /// Windows could not report the descriptor's DACL or SACL.
    ReadAcl,
}

/// A synchronous failure while capturing a caller's security attributes.
#[derive(Debug)]
pub struct SecurityCaptureError {
    failure: SecurityCaptureFailure,
    source: io::Error,
}

impl SecurityCaptureError {
    fn new(failure: SecurityCaptureFailure, source: io::Error) -> Self {
        Self { failure, source }
    }

    fn last_os(failure: SecurityCaptureFailure) -> Self {
        Self::new(failure, io::Error::last_os_error())
    }

    /// Why the capture failed.
    #[must_use]
    pub fn failure(&self) -> SecurityCaptureFailure {
        self.failure
    }

    /// The underlying Win32 error code.
    #[must_use]
    pub fn raw_os_error(&self) -> Option<i32> {
        self.source.raw_os_error()
    }
}

impl fmt::Display for SecurityCaptureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let stage = match self.failure {
            SecurityCaptureFailure::InvalidDescriptor => "IsValidSecurityDescriptor",
            SecurityCaptureFailure::ReadControl => "GetSecurityDescriptorControl",
            SecurityCaptureFailure::SizeSelfRelative => "MakeSelfRelativeSD (sizing)",
            SecurityCaptureFailure::MakeSelfRelative => "MakeSelfRelativeSD",
            SecurityCaptureFailure::InvalidCopy => "IsValidSecurityDescriptor (captured copy)",
            SecurityCaptureFailure::ReadAcl => "GetSecurityDescriptorDacl",
        };

        write!(f, "{stage}: {}", self.source)
    }
}

impl std::error::Error for SecurityCaptureError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

/// What a descriptor says about one of its access-control lists.
///
/// The three states that look alike and are not are kept apart here, because
/// collapsing any pair of them silently changes what the resulting object
/// permits.
///
/// # Example
///
/// The pair that matters most: a **NULL** DACL grants everyone complete access
/// and an **empty** one grants nobody anything. They are opposites, so an
/// `Option<Acl>` that flattened them would not lose detail -- it would invert
/// the grant.
///
/// ```
/// use windows_namespace_request_sys::AclState;
///
/// let absent = AclState::Absent;
/// let null = AclState::Null;
/// let empty = AclState::Empty;
///
/// assert_ne!(null, empty, "NULL allows all; empty allows none");
/// assert_ne!(absent, null);
/// assert_ne!(absent, empty);
///
/// // A populated list also reports how many entries it carries.
/// assert_eq!(AclState::Populated(1), AclState::Populated(1));
/// assert_ne!(AclState::Populated(1), AclState::Populated(2));
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AclState {
    /// The descriptor carries no list at all. The object takes its default.
    Absent,
    /// The descriptor carries a **NULL** list. For a DACL this grants everyone
    /// complete access -- the opposite of what [`Empty`](Self::Empty) does.
    Null,
    /// The descriptor carries a list with no entries. For a DACL this grants
    /// nobody any access.
    Empty,
    /// The descriptor carries a list with this many entries.
    Populated(u32),
}

/// An owned, self-relative copy of a caller's security descriptor.
///
/// Whatever form the caller supplied, this holds the self-relative form in one
/// contiguous, DWORD-aligned blob, so it names nothing outside itself and can
/// be carried to another thread.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SecurityDescriptor {
    blob: AlignedBuffer,
}

impl SecurityDescriptor {
    /// Captures the descriptor at `descriptor`.
    ///
    /// An absolute descriptor is converted to self-relative form; a
    /// self-relative one is copied as it stands. Either way the result owns
    /// every byte it refers to.
    ///
    /// # Errors
    ///
    /// Returns a [`SecurityCaptureError`] when Windows rejects the descriptor
    /// as malformed, or when the conversion fails.
    ///
    /// # Safety
    ///
    /// `descriptor` must be non-null and point to a security descriptor that
    /// stays valid for the duration of this call, including everything an
    /// absolute descriptor points at.
    pub unsafe fn capture(descriptor: *const c_void) -> Result<Self, SecurityCaptureError> {
        let descriptor = descriptor.cast_mut();

        // SAFETY: the caller guarantees a live descriptor.
        if unsafe { IsValidSecurityDescriptor(descriptor) } == FALSE {
            return Err(SecurityCaptureError::last_os(
                SecurityCaptureFailure::InvalidDescriptor,
            ));
        }

        let mut control: SECURITY_DESCRIPTOR_CONTROL = 0;
        let mut revision: u32 = 0;
        // SAFETY: as above; both out-parameters point to writable storage.
        if unsafe { GetSecurityDescriptorControl(descriptor, &raw mut control, &raw mut revision) }
            == FALSE
        {
            return Err(SecurityCaptureError::last_os(
                SecurityCaptureFailure::ReadControl,
            ));
        }

        let blob = if control & SE_SELF_RELATIVE == 0 {
            unsafe { Self::convert_absolute(descriptor) }?
        } else {
            unsafe { Self::copy_self_relative(descriptor) }
        };

        let captured = Self { blob };

        // SAFETY: the blob holds a self-relative descriptor of the length
        // Windows reported or wrote.
        if unsafe { IsValidSecurityDescriptor(captured.as_ptr().cast_mut()) } == FALSE {
            return Err(SecurityCaptureError::last_os(
                SecurityCaptureFailure::InvalidCopy,
            ));
        }

        Ok(captured)
    }

    /// # Safety
    ///
    /// `descriptor` must be a live, valid, self-relative descriptor.
    unsafe fn copy_self_relative(descriptor: PSECURITY_DESCRIPTOR) -> AlignedBuffer {
        // SAFETY: for a self-relative descriptor this reports the length of the
        // whole blob, which is exactly what must be copied.
        let length = unsafe { GetSecurityDescriptorLength(descriptor) } as usize;

        let mut blob = AlignedBuffer::zeroed(length, SELF_RELATIVE_ALIGNMENT);
        // SAFETY: the source is valid for `length` bytes by the contract above,
        // the destination was just allocated with that length, and the two
        // allocations cannot overlap.
        unsafe {
            ptr::copy_nonoverlapping(descriptor.cast::<u8>(), blob.as_mut_ptr(), length);
        }

        blob
    }

    /// # Safety
    ///
    /// `descriptor` must be a live, valid, absolute descriptor, including
    /// everything it points at.
    unsafe fn convert_absolute(
        descriptor: PSECURITY_DESCRIPTOR,
    ) -> Result<AlignedBuffer, SecurityCaptureError> {
        let mut length: u32 = 0;
        // SAFETY: a null destination with a zero length is the documented way
        // to ask for the required size; it is expected to fail.
        let sized = unsafe { MakeSelfRelativeSD(descriptor, ptr::null_mut(), &raw mut length) };
        if sized != FALSE {
            // Windows reported success without being given a buffer, which
            // contradicts its own contract; treat the size as unusable.
            return Err(SecurityCaptureError::new(
                SecurityCaptureFailure::SizeSelfRelative,
                io::Error::from_raw_os_error(
                    i32::try_from(ERROR_INSUFFICIENT_BUFFER)
                        .expect("ERROR_INSUFFICIENT_BUFFER fits in i32"),
                ),
            ));
        }

        let error = io::Error::last_os_error();
        if error.raw_os_error()
            != Some(
                i32::try_from(ERROR_INSUFFICIENT_BUFFER)
                    .expect("ERROR_INSUFFICIENT_BUFFER fits in i32"),
            )
        {
            return Err(SecurityCaptureError::new(
                SecurityCaptureFailure::SizeSelfRelative,
                error,
            ));
        }

        let mut blob = AlignedBuffer::zeroed(length as usize, SELF_RELATIVE_ALIGNMENT);
        // SAFETY: blob is writable for exactly the length Windows asked for.
        let converted = unsafe {
            MakeSelfRelativeSD(
                descriptor,
                blob.as_mut_ptr().cast::<c_void>(),
                &raw mut length,
            )
        };
        if converted == FALSE {
            return Err(SecurityCaptureError::last_os(
                SecurityCaptureFailure::MakeSelfRelative,
            ));
        }

        Ok(blob)
    }

    /// The captured descriptor's address, for handing to a Win32 call.
    ///
    /// The pointer borrows from this value and must not outlive it.
    #[must_use]
    pub fn as_ptr(&self) -> *const c_void {
        self.blob.as_ptr().cast::<c_void>()
    }

    /// The captured descriptor's length in bytes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.blob.len()
    }

    /// Whether the captured descriptor is empty, which a valid one never is.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.blob.is_empty()
    }

    /// The captured descriptor's bytes.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        self.blob.as_slice()
    }

    /// What the descriptor says about its discretionary access-control list.
    ///
    /// # Errors
    ///
    /// Returns a [`SecurityCaptureError`] if Windows will not report the list,
    /// which a descriptor that validated at capture should not do.
    pub fn dacl(&self) -> Result<AclState, SecurityCaptureError> {
        let mut present = FALSE;
        let mut acl: *mut ACL = ptr::null_mut();
        let mut defaulted = FALSE;

        // SAFETY: the blob holds a descriptor that validated at capture, and
        // all three out-parameters point to writable storage.
        let read = unsafe {
            GetSecurityDescriptorDacl(
                self.as_ptr().cast_mut(),
                &raw mut present,
                &raw mut acl,
                &raw mut defaulted,
            )
        };

        Self::acl_state(read, present, acl)
    }

    /// What the descriptor says about its system access-control list.
    ///
    /// # Errors
    ///
    /// As [`dacl`](Self::dacl).
    pub fn sacl(&self) -> Result<AclState, SecurityCaptureError> {
        let mut present = FALSE;
        let mut acl: *mut ACL = ptr::null_mut();
        let mut defaulted = FALSE;

        // SAFETY: as dacl.
        let read = unsafe {
            GetSecurityDescriptorSacl(
                self.as_ptr().cast_mut(),
                &raw mut present,
                &raw mut acl,
                &raw mut defaulted,
            )
        };

        Self::acl_state(read, present, acl)
    }

    fn acl_state(
        read: i32,
        present: i32,
        acl: *const ACL,
    ) -> Result<AclState, SecurityCaptureError> {
        if read == FALSE {
            return Err(SecurityCaptureError::last_os(
                SecurityCaptureFailure::ReadAcl,
            ));
        }

        if present != TRUE {
            return Ok(AclState::Absent);
        }

        // SAFETY: Windows either left the pointer null or pointed it at an ACL
        // inside the descriptor blob this method borrows.
        let Some(acl) = (unsafe { acl.as_ref() }) else {
            return Ok(AclState::Null);
        };

        Ok(match u32::from(acl.AceCount) {
            0 => AclState::Empty,
            count => AclState::Populated(count),
        })
    }
}

/// An owned capture of a caller's `lpSecurityAttributes` argument.
///
/// Three outcomes that a single nullable pointer runs together are kept apart
/// by this type and [`AclState`], because they are three different grants:
///
/// | Caller passed | Meaning | Represented as |
/// |---|---|---|
/// | `NULL` attributes | default security, non-inheritable handle | `None` where a `SecurityAttributes` is expected |
/// | attributes with a `NULL` descriptor | default security, caller's inheritance choice | [`descriptor`](Self::descriptor) is `None` |
/// | attributes with a descriptor | the caller's security | [`descriptor`](Self::descriptor) is `Some` |
///
/// and within a descriptor, an absent DACL, a NULL DACL, and an empty DACL are
/// three further distinct grants, reported by [`SecurityDescriptor::dacl`].
///
/// # Example
///
/// The distinction a single nullable pointer runs together. Passing no
/// attributes at all and passing attributes that carry no descriptor are
/// different requests -- the second still states an inheritance choice:
///
/// ```
/// use windows_namespace_request_sys::SecurityAttributes;
///
/// // Attributes with no descriptor: default security, but the caller's
/// // inheritance choice is still carried.
/// let inheritable = SecurityAttributes::new(None, true);
/// assert!(inheritable.descriptor().is_none());
/// assert!(inheritable.inherit_handle());
///
/// let raw = inheritable.to_raw();
/// assert!(raw.lpSecurityDescriptor.is_null());
/// assert_ne!(raw.bInheritHandle, 0, "the choice survives into the Win32 struct");
///
/// // Passing *no* attributes is the third case, and is spelled `None` where a
/// // `SecurityAttributes` is expected rather than being confused with this.
/// let none: Option<SecurityAttributes> = None;
/// assert!(none.is_none());
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SecurityAttributes {
    descriptor: Option<SecurityDescriptor>,
    inherit_handle: bool,
}

impl SecurityAttributes {
    /// Builds a capture from an already-owned descriptor and an inheritance
    /// choice.
    #[must_use]
    pub fn new(descriptor: Option<SecurityDescriptor>, inherit_handle: bool) -> Self {
        Self {
            descriptor,
            inherit_handle,
        }
    }

    /// Captures the `SECURITY_ATTRIBUTES` at `attributes`.
    ///
    /// A null `attributes` is not an error: it is the caller declining to
    /// supply any, which is reported as `Ok(None)` rather than being confused
    /// with attributes that carry no descriptor.
    ///
    /// # Errors
    ///
    /// Returns a [`SecurityCaptureError`] when the referenced descriptor cannot
    /// be captured.
    ///
    /// # Safety
    ///
    /// `attributes`, if non-null, must point to a `SECURITY_ATTRIBUTES` that
    /// stays valid for the duration of this call, as must any descriptor it
    /// names.
    pub unsafe fn capture(
        attributes: *const SECURITY_ATTRIBUTES,
    ) -> Result<Option<Self>, SecurityCaptureError> {
        // SAFETY: the caller guarantees a live pointer or null.
        let Some(attributes) = (unsafe { attributes.as_ref() }) else {
            return Ok(None);
        };

        let descriptor = if attributes.lpSecurityDescriptor.is_null() {
            None
        } else {
            // SAFETY: non-null, and live for this call by the contract above.
            Some(unsafe { SecurityDescriptor::capture(attributes.lpSecurityDescriptor) }?)
        };

        Ok(Some(Self {
            descriptor,
            inherit_handle: attributes.bInheritHandle != FALSE,
        }))
    }

    /// The captured descriptor, if the caller supplied one.
    #[must_use]
    pub fn descriptor(&self) -> Option<&SecurityDescriptor> {
        self.descriptor.as_ref()
    }

    /// Whether the caller asked for the resulting handle to be inheritable.
    #[must_use]
    pub fn inherit_handle(&self) -> bool {
        self.inherit_handle
    }

    /// Rebuilds the `SECURITY_ATTRIBUTES` to pass to a Win32 call.
    ///
    /// The result points into this value and must not outlive it.
    #[must_use]
    pub fn to_raw(&self) -> SECURITY_ATTRIBUTES {
        SECURITY_ATTRIBUTES {
            nLength: u32::try_from(size_of::<SECURITY_ATTRIBUTES>())
                .expect("SECURITY_ATTRIBUTES is far smaller than u32::MAX"),
            lpSecurityDescriptor: self
                .descriptor
                .as_ref()
                .map_or(ptr::null_mut(), |descriptor| descriptor.as_ptr().cast_mut()),
            bInheritHandle: if self.inherit_handle { TRUE } else { FALSE },
        }
    }
}

// Visible to the crate's own cross-module tests, which reuse this module's
// absolute-descriptor builder rather than standing up a second copy of it.
#[cfg(test)]
pub(crate) mod tests;
