// Copyright (c) Mike Grier.

//! The security descriptor of an open handle: `GetSecurityInfo`.
//!
//! Windows allocates the descriptor it returns with `LocalAlloc`. It is copied into this crate's own
//! owned [`SecurityDescriptor`] and the allocation freed, so nothing the caller holds points at it.

use std::os::windows::io::{AsRawHandle, BorrowedHandle};
use std::ptr;

use win_error_codes::Win32Error;
use windows_sys::Win32::Foundation::{ERROR_INVALID_DATA, ERROR_SUCCESS};
use windows_sys::Win32::Security::Authorization::{
    GetSecurityInfo, SE_DS_OBJECT, SE_DS_OBJECT_ALL, SE_FILE_OBJECT, SE_KERNEL_OBJECT, SE_LMSHARE,
    SE_OBJECT_TYPE, SE_PRINTER, SE_PROVIDER_DEFINED_OBJECT, SE_REGISTRY_KEY,
    SE_REGISTRY_WOW64_32KEY, SE_REGISTRY_WOW64_64KEY, SE_SERVICE, SE_UNKNOWN_OBJECT_TYPE,
    SE_WINDOW_OBJECT, SE_WMIGUID_OBJECT,
};
use windows_sys::Win32::Security::PSECURITY_DESCRIPTOR;

use super::sddl::LocalAllocation;
use super::{
    SecurityCaptureError, SecurityCaptureFailure, SecurityDescriptor, SecurityInformation,
};

#[cfg(test)]
mod tests;

/// The kind of object a handle names, which `GetSecurityInfo` needs to know to find its security.
///
/// A newtype over `SE_OBJECT_TYPE`, because Windows defines more kinds than are named here and this
/// crate does not narrow what the call accepts. Which kinds a handle can be asked about, and what is
/// answered for a kind that does not match the handle, is Windows' to say.
///
/// # Example
///
/// ```
/// use win_authorization_sys::ObjectType;
///
/// assert_ne!(ObjectType::FILE, ObjectType::KERNEL_OBJECT);
/// assert_eq!(ObjectType::from_raw(ObjectType::FILE.as_raw()), ObjectType::FILE);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ObjectType(SE_OBJECT_TYPE);

impl ObjectType {
    /// `SE_UNKNOWN_OBJECT_TYPE`.
    pub const UNKNOWN: Self = Self(SE_UNKNOWN_OBJECT_TYPE);
    /// `SE_FILE_OBJECT`: a file or directory, or a named pipe or other object a file handle names.
    pub const FILE: Self = Self(SE_FILE_OBJECT);
    /// `SE_SERVICE`: a Windows service.
    pub const SERVICE: Self = Self(SE_SERVICE);
    /// `SE_PRINTER`: a printer.
    pub const PRINTER: Self = Self(SE_PRINTER);
    /// `SE_REGISTRY_KEY`: a registry key.
    pub const REGISTRY_KEY: Self = Self(SE_REGISTRY_KEY);
    /// `SE_LMSHARE`: a network share.
    pub const LMSHARE: Self = Self(SE_LMSHARE);
    /// `SE_KERNEL_OBJECT`: a kernel object that has no kind of its own here, such as an event, a
    /// mutex, a semaphore, a process or a thread.
    pub const KERNEL_OBJECT: Self = Self(SE_KERNEL_OBJECT);
    /// `SE_WINDOW_OBJECT`: a window station or desktop.
    pub const WINDOW_OBJECT: Self = Self(SE_WINDOW_OBJECT);
    /// `SE_DS_OBJECT`: a directory service object.
    pub const DS_OBJECT: Self = Self(SE_DS_OBJECT);
    /// `SE_DS_OBJECT_ALL`: a directory service object and its properties.
    pub const DS_OBJECT_ALL: Self = Self(SE_DS_OBJECT_ALL);
    /// `SE_PROVIDER_DEFINED_OBJECT`: an object of a provider's own kind.
    pub const PROVIDER_DEFINED: Self = Self(SE_PROVIDER_DEFINED_OBJECT);
    /// `SE_WMIGUID_OBJECT`: a WMI object.
    pub const WMIGUID: Self = Self(SE_WMIGUID_OBJECT);
    /// `SE_REGISTRY_WOW64_32KEY`: a registry key in the 32-bit view.
    pub const REGISTRY_WOW64_32KEY: Self = Self(SE_REGISTRY_WOW64_32KEY);
    /// `SE_REGISTRY_WOW64_64KEY`: a registry key in the 64-bit view.
    pub const REGISTRY_WOW64_64KEY: Self = Self(SE_REGISTRY_WOW64_64KEY);

    /// A kind from its Win32 value.
    #[must_use]
    pub const fn from_raw(raw: SE_OBJECT_TYPE) -> Self {
        Self(raw)
    }

    /// The kind's Win32 value.
    #[must_use]
    pub const fn as_raw(self) -> SE_OBJECT_TYPE {
        self.0
    }
}

/// The parts `information` selects of the security descriptor of the object `handle` names, as an
/// owned [`SecurityDescriptor`]: `GetSecurityInfo`.
///
/// What is read is what the handle was opened with the right to read: `READ_CONTROL` for the owner,
/// the group and the DACL, and `ACCESS_SYSTEM_SECURITY` for the SACL, which Windows grants only to a
/// caller holding the privilege for it. A part the handle cannot read is Windows' refusal, with its
/// code, and not an absent part: nothing here retries with less. Windows reads a handle's security
/// whichever of the file, kernel-object and window kinds is named (measured), so the kind does not
/// make a handle of another kind an error, and an object that has no descriptor reads as one with
/// no parts. A part that was not selected is
/// absent from the result, and one that was and the object does not have reads as an absent list
/// ([`AclState::Absent`](crate::AclState::Absent)).
///
/// # Example
///
/// The DACL a file was created with, read back through an open handle:
///
/// ```
/// use std::fs::File;
/// use std::os::windows::io::AsHandle;
///
/// use win_authorization_sys::{get_security_info, AclState, ObjectType, SecurityInformation};
///
/// let path = std::env::temp_dir().join(format!("wa-doc-{}.tmp", std::process::id()));
/// let file = File::create(&path)?;
///
/// let descriptor = get_security_info(file.as_handle(), ObjectType::FILE, SecurityInformation::DACL)?;
/// assert!(matches!(descriptor.dacl()?, AclState::Populated(_)));
/// # drop(file);
/// # std::fs::remove_file(&path)?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
///
/// # Errors
///
/// [`SecurityCaptureFailure::ReadSecurity`] with Windows' own code, unaltered, when it refuses the
/// query: `ERROR_ACCESS_DENIED` for a handle opened without the right to read what was selected,
/// `ERROR_INVALID_PARAMETER` for [`ObjectType::UNKNOWN`]. Otherwise the failures of capturing the descriptor Windows
/// returned, including [`SecurityCaptureFailure::Allocate`] when the memory for the copy cannot be
/// allocated.
pub fn get_security_info(
    handle: BorrowedHandle<'_>,
    object_type: ObjectType,
    information: SecurityInformation,
) -> Result<SecurityDescriptor, SecurityCaptureError> {
    let mut descriptor: PSECURITY_DESCRIPTOR = ptr::null_mut();
    // SAFETY: the handle is open for the borrow, the four parts that would be returned by pointer
    // are not asked for, and `descriptor` is a writable pointer for the allocation Windows makes.
    let status = unsafe {
        GetSecurityInfo(
            handle.as_raw_handle(),
            object_type.as_raw(),
            information.bits(),
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            &raw mut descriptor,
        )
    };
    // Freed on every path below, including the capture's failure.
    let allocation = LocalAllocation(descriptor);

    if status != ERROR_SUCCESS {
        return Err(SecurityCaptureError::new(
            SecurityCaptureFailure::ReadSecurity,
            Win32Error::from_code(status).to_io_error(),
        ));
    }
    // Windows documents a descriptor on success, so this is not reached; it stands between a
    // violation of that and the dereference the capture would make of it, and no test can reach it.
    if allocation.0.is_null() {
        return Err(SecurityCaptureError::new(
            SecurityCaptureFailure::ReadSecurity,
            Win32Error::from_code(ERROR_INVALID_DATA).to_io_error(),
        ));
    }

    // SAFETY: Windows returned a valid self-relative descriptor in memory that stays allocated until
    // `allocation` is dropped, after the capture has copied out of it.
    unsafe { SecurityDescriptor::capture(allocation.0) }
}
