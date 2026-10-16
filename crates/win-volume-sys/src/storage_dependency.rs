// Copyright (c) 2026 Mike Grier
//! How a virtual disk relates to the file that backs it: `GetStorageDependencyInformation`.
//!
//! A volume on a VHD or VHDX has no disk of its own to flush; its writes land in a file on another
//! volume, whose disks are the ones that matter, so this is the call that names that other volume.
//!
//! # Which way it looks
//!
//! Without [`StorageDependencyFlags::HOST_VOLUMES`] the call describes the volumes or disks the
//! handle's volume **hosts**: for the volume holding `C:\file.vhd`, the virtual volume. With it, the
//! call describes the volumes that **host** the handle's volume: for the virtual volume, `C:`. The
//! second is the direction a flush domain follows. [`StorageDependencyFlags::DISK_HANDLE`] says the
//! handle is to a disk and not a volume or root directory.
//!
//! # What it takes and what it returns
//!
//! The reply is a `STORAGE_DEPENDENCY_INFO`: a version, a count, and that many entries of the
//! version asked for. **The version is an input**: the call fails with `ERROR_INVALID_LEVEL` unless
//! the buffer's version field is 1 or 2 when it is made, so the function sets it, and the version a
//! caller asks for decides which fields an entry has. Version 2 adds the ancestor level and four
//! names; the names are pointers, which this module reads only where they lie within the reply and
//! copies out, so nothing a caller holds points into the buffer.
//!
//! The return value is the Win32 error code itself, not `BOOL` followed by `GetLastError`. A handle
//! that is not on a virtual disk fails with `ERROR_VIRTDISK_NOT_VIRTUAL_DISK` (`0xC03A0015`), a
//! named Win32 error whose value is 2^31 or more. It is carried **unaltered** in the
//! [`io::Error`], exactly, through [`win_error_codes::Win32Error`].
//!
//! # What was and was not observed
//!
//! Measured on a machine that is not elevated and whose one disk is a Hyper-V guest's, with no VHD
//! attached from the host: a volume, a root directory or a disk handle that hosts nothing answers
//! success with **zero entries**, the version echoed back; a buffer smaller than the structure's own
//! size (72 bytes, for both versions) is `ERROR_INVALID_PARAMETER` and writes nothing, not even
//! `SizeUsed`; a version other than 1 or 2 is `ERROR_INVALID_LEVEL`; and `HOST_VOLUMES` on a handle
//! that is not on a virtual disk is `0xC03A0015`.
//!
//! **Never observed:** an entry. Attaching a VHD needs an elevated token, which was not available.
//! So the entries' layout is the documented one and is tested against replies built here; that the
//! name pointers point *into the buffer given* (the documentation does not say, but a variable-length
//! structure with a size parameter implies it) is unverified, and a name that lies outside the reply is
//! [`io::ErrorKind::InvalidData`] and not read; and what a buffer too small for the entries returns is
//! unverified, so the call is asked again at `SizeUsed` on `ERROR_INSUFFICIENT_BUFFER` or
//! `ERROR_MORE_DATA`, and [`storage_dependency_information_with_capacity`] sets the first size for a
//! caller who would rather not rely on that.

use std::io;
use std::mem::offset_of;
use std::ops::BitOr;
use std::os::windows::io::{AsHandle, AsRawHandle};

use io_buffers::AlignedBuf;
use win_error_codes::Win32Error;
use windows_sys::Win32::Foundation::{
    ERROR_INSUFFICIENT_BUFFER, ERROR_MORE_DATA, ERROR_NOT_ENOUGH_MEMORY, ERROR_SUCCESS,
};
use windows_sys::Win32::Storage::Vhd::{
    DEPENDENT_DISK_FLAG_ALWAYS_ALLOW_SPARSE, DEPENDENT_DISK_FLAG_FULLY_ALLOCATED,
    DEPENDENT_DISK_FLAG_MULT_BACKING_FILES, DEPENDENT_DISK_FLAG_NO_DRIVE_LETTER,
    DEPENDENT_DISK_FLAG_NO_HOST_DISK, DEPENDENT_DISK_FLAG_NONE, DEPENDENT_DISK_FLAG_PARENT,
    DEPENDENT_DISK_FLAG_PERMANENT_LIFETIME, DEPENDENT_DISK_FLAG_READ_ONLY,
    DEPENDENT_DISK_FLAG_REMOTE, DEPENDENT_DISK_FLAG_REMOVABLE,
    DEPENDENT_DISK_FLAG_SUPPORT_COMPRESSED_VOLUMES, DEPENDENT_DISK_FLAG_SUPPORT_ENCRYPTED_FILES,
    DEPENDENT_DISK_FLAG_SYSTEM_VOLUME, DEPENDENT_DISK_FLAG_SYSTEM_VOLUME_PARENT,
    GET_STORAGE_DEPENDENCY_FLAG, GET_STORAGE_DEPENDENCY_FLAG_DISK_HANDLE,
    GET_STORAGE_DEPENDENCY_FLAG_HOST_VOLUMES, GET_STORAGE_DEPENDENCY_FLAG_NONE,
    GetStorageDependencyInformation, STORAGE_DEPENDENCY_INFO, STORAGE_DEPENDENCY_INFO_TYPE_1,
    STORAGE_DEPENDENCY_INFO_TYPE_2, STORAGE_DEPENDENCY_INFO_VERSION_1,
    STORAGE_DEPENDENCY_INFO_VERSION_2, VIRTUAL_STORAGE_TYPE,
};
use wtf_string::Wtf16String;

use crate::device_control::{invalid_data, u32_at};

#[cfg(test)]
mod tests;

/// How many bytes the first attempt has room for.
///
/// A version 2 entry is a few dozen bytes and a name a few dozen characters, so a few entries fit; a
/// larger reply is asked for again at the size the call reports.
const FIRST_BYTES: usize = 4096;

// The claim above, checked at build time: the structure and two version 2 entries fit.
const _: () = assert!(
    FIRST_BYTES
        >= size_of::<STORAGE_DEPENDENCY_INFO>() + 2 * size_of::<STORAGE_DEPENDENCY_INFO_TYPE_2>()
);

/// How many times the buffer is grown before the reply is called unstable.
const MAX_ATTEMPTS: usize = 4;

/// The smallest buffer the call accepts: the structure's own size, which a buffer of any smaller
/// size is refused at as `ERROR_INVALID_PARAMETER` (measured).
const MIN_BYTES: usize = size_of::<STORAGE_DEPENDENCY_INFO>();

/// The alignment the reply needs: the structure holds pointers.
const ALIGNMENT: usize = align_of::<STORAGE_DEPENDENCY_INFO>();

/// Where the version is, which is also where the caller's request for one is made.
const VERSION_AT: usize = offset_of!(STORAGE_DEPENDENCY_INFO, Version);

/// Which way a dependency query looks, and what kind of handle it was given.
///
/// A newtype over `GET_STORAGE_DEPENDENCY_FLAG`, because the value is a combination and Windows may
/// define more.
///
/// # Example
///
/// ```
/// use win_volume_sys::StorageDependencyFlags;
///
/// // The volumes that host a volume, from a handle to a disk.
/// let flags = StorageDependencyFlags::HOST_VOLUMES | StorageDependencyFlags::DISK_HANDLE;
/// assert!(flags.contains(StorageDependencyFlags::HOST_VOLUMES));
/// assert!(flags.contains(StorageDependencyFlags::DISK_HANDLE));
/// assert!(!StorageDependencyFlags::NONE.contains(StorageDependencyFlags::HOST_VOLUMES));
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct StorageDependencyFlags(GET_STORAGE_DEPENDENCY_FLAG);

impl StorageDependencyFlags {
    /// No flags: the volumes or disks the handle's volume hosts, from a volume or root directory.
    pub const NONE: Self = Self(GET_STORAGE_DEPENDENCY_FLAG_NONE);

    /// `GET_STORAGE_DEPENDENCY_FLAG_HOST_VOLUMES`: the volumes or disks that host the handle's
    /// volume.
    pub const HOST_VOLUMES: Self = Self(GET_STORAGE_DEPENDENCY_FLAG_HOST_VOLUMES);

    /// `GET_STORAGE_DEPENDENCY_FLAG_DISK_HANDLE`: the handle is to a disk, not a volume or file.
    pub const DISK_HANDLE: Self = Self(GET_STORAGE_DEPENDENCY_FLAG_DISK_HANDLE);

    /// Flags from their raw bits, for a value Windows defines that this crate has no name for.
    #[must_use]
    pub const fn from_bits(bits: GET_STORAGE_DEPENDENCY_FLAG) -> Self {
        Self(bits)
    }

    /// The raw bits.
    #[must_use]
    pub const fn bits(self) -> GET_STORAGE_DEPENDENCY_FLAG {
        self.0
    }

    /// Whether every bit of `other` is set. Everything contains [`NONE`](Self::NONE).
    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

impl BitOr for StorageDependencyFlags {
    type Output = Self;

    fn bitor(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

/// Which version of the reply to ask for.
///
/// Version 1 entries carry the type flags and the virtual storage type. Version 2 adds the ancestor
/// level and four names. Windows refuses any other value with `ERROR_INVALID_LEVEL`, and
/// [`from_raw`](Self::from_raw) passes one through so that is the device's to say.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct StorageDependencyVersion(i32);

impl StorageDependencyVersion {
    /// `STORAGE_DEPENDENCY_INFO_VERSION_1`.
    pub const V1: Self = Self(STORAGE_DEPENDENCY_INFO_VERSION_1);

    /// `STORAGE_DEPENDENCY_INFO_VERSION_2`, which is what a query asks for unless told otherwise.
    pub const V2: Self = Self(STORAGE_DEPENDENCY_INFO_VERSION_2);

    /// A version, as the call takes it.
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

/// What a dependency is, as the flags of `DEPENDENT_DISK_FLAG`.
///
/// A newtype over the raw bits, because the value is a combination and Windows may define more.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct DependentDiskFlags(i32);

impl DependentDiskFlags {
    /// `DEPENDENT_DISK_FLAG_NONE`.
    pub const NONE: Self = Self(DEPENDENT_DISK_FLAG_NONE);
    /// `DEPENDENT_DISK_FLAG_MULT_BACKING_FILES`.
    pub const MULT_BACKING_FILES: Self = Self(DEPENDENT_DISK_FLAG_MULT_BACKING_FILES);
    /// `DEPENDENT_DISK_FLAG_FULLY_ALLOCATED`.
    pub const FULLY_ALLOCATED: Self = Self(DEPENDENT_DISK_FLAG_FULLY_ALLOCATED);
    /// `DEPENDENT_DISK_FLAG_READ_ONLY`.
    pub const READ_ONLY: Self = Self(DEPENDENT_DISK_FLAG_READ_ONLY);
    /// `DEPENDENT_DISK_FLAG_REMOTE`.
    pub const REMOTE: Self = Self(DEPENDENT_DISK_FLAG_REMOTE);
    /// `DEPENDENT_DISK_FLAG_SYSTEM_VOLUME`.
    pub const SYSTEM_VOLUME: Self = Self(DEPENDENT_DISK_FLAG_SYSTEM_VOLUME);
    /// `DEPENDENT_DISK_FLAG_SYSTEM_VOLUME_PARENT`.
    pub const SYSTEM_VOLUME_PARENT: Self = Self(DEPENDENT_DISK_FLAG_SYSTEM_VOLUME_PARENT);
    /// `DEPENDENT_DISK_FLAG_REMOVABLE`.
    pub const REMOVABLE: Self = Self(DEPENDENT_DISK_FLAG_REMOVABLE);
    /// `DEPENDENT_DISK_FLAG_NO_DRIVE_LETTER`.
    pub const NO_DRIVE_LETTER: Self = Self(DEPENDENT_DISK_FLAG_NO_DRIVE_LETTER);
    /// `DEPENDENT_DISK_FLAG_PARENT`.
    pub const PARENT: Self = Self(DEPENDENT_DISK_FLAG_PARENT);
    /// `DEPENDENT_DISK_FLAG_NO_HOST_DISK`.
    pub const NO_HOST_DISK: Self = Self(DEPENDENT_DISK_FLAG_NO_HOST_DISK);
    /// `DEPENDENT_DISK_FLAG_PERMANENT_LIFETIME`.
    pub const PERMANENT_LIFETIME: Self = Self(DEPENDENT_DISK_FLAG_PERMANENT_LIFETIME);
    /// `DEPENDENT_DISK_FLAG_SUPPORT_COMPRESSED_VOLUMES`.
    pub const SUPPORT_COMPRESSED_VOLUMES: Self =
        Self(DEPENDENT_DISK_FLAG_SUPPORT_COMPRESSED_VOLUMES);
    /// `DEPENDENT_DISK_FLAG_ALWAYS_ALLOW_SPARSE`.
    pub const ALWAYS_ALLOW_SPARSE: Self = Self(DEPENDENT_DISK_FLAG_ALWAYS_ALLOW_SPARSE);
    /// `DEPENDENT_DISK_FLAG_SUPPORT_ENCRYPTED_FILES`.
    pub const SUPPORT_ENCRYPTED_FILES: Self = Self(DEPENDENT_DISK_FLAG_SUPPORT_ENCRYPTED_FILES);

    /// Flags from their raw bits.
    #[must_use]
    pub const fn from_bits(bits: i32) -> Self {
        Self(bits)
    }

    /// The raw bits, as the device wrote them.
    #[must_use]
    pub const fn bits(self) -> i32 {
        self.0
    }

    /// Whether every bit of `other` is set. Everything contains [`NONE`](Self::NONE).
    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

impl BitOr for DependentDiskFlags {
    type Output = Self;

    fn bitor(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

/// The kind of virtual storage an entry is: a device id and the vendor that defines it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct VirtualStorageType {
    device_id: u32,
    vendor_id: [u8; 16],
}

impl VirtualStorageType {
    /// The device id (`VIRTUAL_STORAGE_TYPE_DEVICE_VHD` and the rest), as the device wrote it.
    #[must_use]
    pub fn device_id(&self) -> u32 {
        self.device_id
    }

    /// The vendor's GUID, as the sixteen bytes it has in memory.
    #[must_use]
    pub fn vendor_id(&self) -> [u8; 16] {
        self.vendor_id
    }
}

/// One dependency: a virtual disk, or a volume that hosts one.
///
/// The names exist only in version 2 replies, and are `None` for a version 1 entry and for a name
/// the call left null. They are copied out of the reply and owned.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct StorageDependency {
    type_flags: DependentDiskFlags,
    provider_specific_flags: u32,
    virtual_storage_type: VirtualStorageType,
    ancestor_level: Option<u32>,
    dependency_device_name: Option<Wtf16String>,
    host_volume_name: Option<Wtf16String>,
    dependent_volume_name: Option<Wtf16String>,
    dependent_volume_relative_path: Option<Wtf16String>,
}

impl StorageDependency {
    /// What the dependency is.
    #[must_use]
    pub fn dependency_type_flags(&self) -> DependentDiskFlags {
        self.type_flags
    }

    /// Flags specific to the virtual disk provider, as the device wrote them.
    #[must_use]
    pub fn provider_specific_flags(&self) -> u32 {
        self.provider_specific_flags
    }

    /// The kind of virtual storage.
    #[must_use]
    pub fn virtual_storage_type(&self) -> VirtualStorageType {
        self.virtual_storage_type
    }

    /// The ancestor level. Version 2 only.
    #[must_use]
    pub fn ancestor_level(&self) -> Option<u32> {
        self.ancestor_level
    }

    /// The device name of the dependent device, in the documented form `\\.\PhysicalDriveN` for a
    /// virtual hard disk or `\\.\CDRomN` for an ISO. Version 2 only.
    #[must_use]
    pub fn dependency_device_name(&self) -> Option<&Wtf16String> {
        self.dependency_device_name.as_ref()
    }

    /// The host volume, in the documented form `\\?\Volume{GUID}\`. Version 2 only.
    #[must_use]
    pub fn host_volume_name(&self) -> Option<&Wtf16String> {
        self.host_volume_name.as_ref()
    }

    /// The dependent volume, if any, in the documented form `\\?\Volume{GUID}\`. Version 2 only.
    #[must_use]
    pub fn dependent_volume_name(&self) -> Option<&Wtf16String> {
        self.dependent_volume_name.as_ref()
    }

    /// The relative path to the dependent volume. Version 2 only.
    #[must_use]
    pub fn dependent_volume_relative_path(&self) -> Option<&Wtf16String> {
        self.dependent_volume_relative_path.as_ref()
    }
}

/// What a dependency query returned: the version the reply is in, and its entries.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct StorageDependencyInfo {
    version: StorageDependencyVersion,
    entries: Vec<StorageDependency>,
}

impl StorageDependencyInfo {
    /// The version of the reply, as the call echoed it.
    #[must_use]
    pub fn version(&self) -> StorageDependencyVersion {
        self.version
    }

    /// The entries, in the order the call gave them. Empty for a volume or disk that hosts nothing,
    /// and for one that is hosted by nothing, which is success and not a failure.
    #[must_use]
    pub fn entries(&self) -> &[StorageDependency] {
        &self.entries
    }
}

/// The dependencies of the volume `handle` names, in the version `version`, looking the way `flags`
/// says: for the volume that hosts a virtual disk, the virtual volume, and with
/// [`StorageDependencyFlags::HOST_VOLUMES`] the other way round. The call is asked again at the size
/// it reports if the first buffer was too small.
///
/// `handle` is a handle to a volume, a root directory, or with [`StorageDependencyFlags::DISK_HANDLE`]
/// a disk, and needs no access to it: a handle opened with none is enough, which an ordinary account
/// can open. Without [`StorageDependencyFlags::HOST_VOLUMES`], a volume that hosts nothing is not an
/// error to ask about: the entries are empty. With it, a volume that is not on a virtual disk is
/// `ERROR_VIRTDISK_NOT_VIRTUAL_DISK`, the call's own answer that there is no host to name.
///
/// # Example
///
/// The volume that hosts the volume `handle` names, which for a volume on a VHD is the volume
/// holding the file. A volume that is not on a virtual disk, such as an ordinary `C:`, has none, and
/// the call says so with `ERROR_VIRTDISK_NOT_VIRTUAL_DISK`:
///
/// ```no_run
/// use std::fs::OpenOptions;
/// use std::os::windows::fs::OpenOptionsExt;
///
/// use win_volume_sys::{
///     StorageDependencyFlags, StorageDependencyVersion, storage_dependency_information,
/// };
///
/// /// `ERROR_VIRTDISK_NOT_VIRTUAL_DISK`.
/// const NOT_VIRTUAL_DISK: u32 = 0xC03A_0015;
///
/// let volume = OpenOptions::new()
///     .access_mode(0)
///     .share_mode(7)
///     .open(r"\\.\C:")?;
///
/// match storage_dependency_information(
///     &volume,
///     StorageDependencyFlags::HOST_VOLUMES,
///     StorageDependencyVersion::V2,
/// ) {
///     Ok(info) => {
///         for entry in info.entries() {
///             println!("{:?}", entry.host_volume_name());
///         }
///     }
///     // Not on a virtual disk: nothing hosts it.
///     Err(error) if error.raw_os_error() == Some(NOT_VIRTUAL_DISK.cast_signed()) => {}
///     Err(error) => return Err(error),
/// }
/// # Ok::<(), std::io::Error>(())
/// ```
///
/// # Errors
///
/// The call's own status, with its code exactly: `ERROR_VIRTDISK_NOT_VIRTUAL_DISK` for
/// `HOST_VOLUMES` on a handle that is not on a virtual disk, `ERROR_INVALID_LEVEL` for a version
/// that is not 1 or 2, `ERROR_INVALID_PARAMETER` for flags that do not fit the handle.
/// [`io::ErrorKind::InvalidData`] for a reply that does not hold what it says, a version this
/// module does not know, and a size that kept changing between the calls. A buffer that cannot be
/// allocated for a size the call named is `ERROR_NOT_ENOUGH_MEMORY`, and the call is not made again.
pub fn storage_dependency_information(
    handle: &impl AsHandle,
    flags: StorageDependencyFlags,
    version: StorageDependencyVersion,
) -> io::Result<StorageDependencyInfo> {
    storage_dependency_information_with_capacity(handle, flags, version, FIRST_BYTES)
}

/// [`storage_dependency_information`] with the size of the first buffer set to `first_bytes`. A size
/// below the structure's own is raised to it, because the call refuses less.
///
/// The default is enough for a few entries. A larger reply is asked for again at the size the call
/// reports, which is unverified; a caller who would rather not rely on that sets this large enough.
///
/// # Errors
///
/// As for [`storage_dependency_information`].
pub fn storage_dependency_information_with_capacity(
    handle: &impl AsHandle,
    flags: StorageDependencyFlags,
    version: StorageDependencyVersion,
    first_bytes: usize,
) -> io::Result<StorageDependencyInfo> {
    let handle = handle.as_handle().as_raw_handle();
    dependencies_via(first_bytes, version, |buffer, used| {
        let size = u32::try_from(buffer.len()).unwrap_or(u32::MAX);
        // SAFETY: `handle` is held open by the caller's borrow for the whole call. `buffer` is
        // writable for `size` bytes, aligned for the structure it receives, and outlives the call,
        // which does not keep a pointer into it; and `used` is a valid `u32` for the call to write.
        unsafe {
            GetStorageDependencyInformation(
                handle,
                flags.bits(),
                size,
                buffer.as_mut_ptr().cast(),
                used,
            )
        }
    })
}

/// The reply `ask` gives, which fills the buffer it is lent, writes how many bytes it used, and
/// returns the call's status.
///
/// Separate from the call so that replies with entries, which an unelevated machine with no virtual
/// disk cannot produce, can be had from a test.
fn dependencies_via(
    first: usize,
    version: StorageDependencyVersion,
    mut ask: impl FnMut(&mut [u8], *mut u32) -> u32,
) -> io::Result<StorageDependencyInfo> {
    let mut capacity = first.max(MIN_BYTES);

    for _ in 0..MAX_ATTEMPTS {
        let mut buffer = AlignedBuf::zeroed(capacity, ALIGNMENT)
            .map_err(|_| Win32Error::from_code(ERROR_NOT_ENOUGH_MEMORY).to_io_error())?;
        buffer.as_mut_slice()[VERSION_AT..VERSION_AT + size_of::<i32>()]
            .copy_from_slice(&version.raw().to_ne_bytes());

        let mut used = 0_u32;
        let status = ask(buffer.as_mut_slice(), &raw mut used);

        match status {
            ERROR_SUCCESS => return decode(buffer.as_slice(), used),
            ERROR_INSUFFICIENT_BUFFER | ERROR_MORE_DATA => {
                let needed = usize::try_from(used).unwrap_or(usize::MAX);
                if needed <= capacity {
                    return Err(invalid_data(
                        "a reply that needs more room names a size already given",
                    ));
                }
                capacity = needed;
            }
            other => return Err(Win32Error::from_code(other).to_io_error()),
        }
    }

    Err(invalid_data("a reply whose size kept changing"))
}

/// Reads the first `used` bytes of `buffer`, which the call filled: the version, the count, and
/// that many entries of that version.
///
/// Every offset and every name pointer is checked against the bytes the call says it used, so a
/// damaged reply is an error and never a read outside them. The name pointers are absolute
/// addresses; they are compared with the address the reply is at and read as offsets into it, so
/// no pointer is followed.
fn decode(buffer: &[u8], used: u32) -> io::Result<StorageDependencyInfo> {
    let malformed = invalid_data;

    let used = usize::try_from(used)
        .ok()
        .filter(|used| *used <= buffer.len())
        .ok_or_else(|| malformed("a call that used more than it was given room for"))?;
    let reply = &buffer[..used];

    let raw_version =
        u32_at(reply, VERSION_AT).ok_or_else(|| malformed("a reply with no version"))?;
    let version = i32::try_from(raw_version)
        .map(StorageDependencyVersion::from_raw)
        .map_err(|_| malformed("a version too large to be one"))?;
    let count = u32_at(reply, offset_of!(STORAGE_DEPENDENCY_INFO, NumberEntries))
        .ok_or_else(|| malformed("a reply with no count"))?;
    let count = usize::try_from(count).map_err(|_| malformed("a count too large to hold"))?;

    let entry_size = if version == StorageDependencyVersion::V1 {
        size_of::<STORAGE_DEPENDENCY_INFO_TYPE_1>()
    } else if version == StorageDependencyVersion::V2 {
        size_of::<STORAGE_DEPENDENCY_INFO_TYPE_2>()
    } else {
        return Err(malformed("a reply in a version this entry does not know"));
    };
    let first = offset_of!(STORAGE_DEPENDENCY_INFO, Anonymous);
    count
        .checked_mul(entry_size)
        .and_then(|entries| entries.checked_add(first))
        .filter(|end| *end <= reply.len())
        .ok_or_else(|| malformed("a count the reply cannot hold"))?;

    let base = reply.as_ptr().addr();
    let entries = (0..count)
        .map(|index| decode_entry(reply, base, version, first + index * entry_size))
        .collect::<Result<Vec<_>, _>>()?;

    Ok(StorageDependencyInfo { version, entries })
}

/// The entry at `at`, which the caller has checked lies within `reply`.
fn decode_entry(
    reply: &[u8],
    base: usize,
    version: StorageDependencyVersion,
    at: usize,
) -> io::Result<StorageDependency> {
    let malformed = invalid_data;
    let read =
        |offset: usize| u32_at(reply, at + offset).ok_or_else(|| malformed("an entry cut short"));

    let type_flags = read(offset_of!(
        STORAGE_DEPENDENCY_INFO_TYPE_1,
        DependencyTypeFlags
    ))?;
    let provider_specific_flags = read(offset_of!(
        STORAGE_DEPENDENCY_INFO_TYPE_1,
        ProviderSpecificFlags
    ))?;
    let storage_type_at = at + offset_of!(STORAGE_DEPENDENCY_INFO_TYPE_1, VirtualStorageType);
    let device_id = u32_at(
        reply,
        storage_type_at + offset_of!(VIRTUAL_STORAGE_TYPE, DeviceId),
    )
    .ok_or_else(|| malformed("an entry cut short"))?;
    let vendor_at = storage_type_at + offset_of!(VIRTUAL_STORAGE_TYPE, VendorId);
    let vendor_id: [u8; 16] = reply
        .get(vendor_at..vendor_at + 16)
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or_else(|| malformed("an entry cut short"))?;

    let mut entry = StorageDependency {
        type_flags: DependentDiskFlags::from_bits(i32::from_ne_bytes(type_flags.to_ne_bytes())),
        provider_specific_flags,
        virtual_storage_type: VirtualStorageType {
            device_id,
            vendor_id,
        },
        ancestor_level: None,
        dependency_device_name: None,
        host_volume_name: None,
        dependent_volume_name: None,
        dependent_volume_relative_path: None,
    };

    if version == StorageDependencyVersion::V2 {
        let pointer = |offset: usize| -> io::Result<usize> {
            let bytes = reply
                .get(at + offset..at + offset + size_of::<usize>())
                .ok_or_else(|| malformed("an entry cut short"))?;
            Ok(usize::from_ne_bytes(
                bytes
                    .try_into()
                    .map_err(|_| malformed("an entry cut short"))?,
            ))
        };
        let name = |offset: usize| -> io::Result<Option<Wtf16String>> {
            read_name(reply, base, pointer(offset)?)
        };

        entry.ancestor_level = Some(read(offset_of!(
            STORAGE_DEPENDENCY_INFO_TYPE_2,
            AncestorLevel
        ))?);
        entry.dependency_device_name = name(offset_of!(
            STORAGE_DEPENDENCY_INFO_TYPE_2,
            DependencyDeviceName
        ))?;
        entry.host_volume_name = name(offset_of!(STORAGE_DEPENDENCY_INFO_TYPE_2, HostVolumeName))?;
        entry.dependent_volume_name = name(offset_of!(
            STORAGE_DEPENDENCY_INFO_TYPE_2,
            DependentVolumeName
        ))?;
        entry.dependent_volume_relative_path = name(offset_of!(
            STORAGE_DEPENDENCY_INFO_TYPE_2,
            DependentVolumeRelativePath
        ))?;
    }

    Ok(entry)
}

/// The NUL-terminated UTF-16 string a name pointer addresses, copied out, or `None` for a null
/// pointer.
///
/// The pointer must address a position inside `reply`, whose first byte is at address `base`, and
/// the string must end inside it. Anything else is reported and not read.
fn read_name(reply: &[u8], base: usize, pointer: usize) -> io::Result<Option<Wtf16String>> {
    let malformed = invalid_data;

    if pointer == 0 {
        return Ok(None);
    }
    let offset = pointer
        .checked_sub(base)
        .filter(|offset| *offset < reply.len())
        .ok_or_else(|| malformed("a name that does not lie within the reply"))?;

    let units = reply[offset..]
        .as_chunks::<{ size_of::<u16>() }>()
        .0
        .iter()
        .map(|unit| u16::from_ne_bytes(*unit))
        .take_while(|unit| *unit != 0)
        .collect::<Vec<_>>();
    // `take_while` also stops at the end of the bytes, so the terminator is checked for by length:
    // a string that ran to the end of the reply has no room left for one.
    let terminated = offset + (units.len() + 1) * size_of::<u16>() <= reply.len();
    if !terminated {
        return Err(malformed("a name with no terminator within the reply"));
    }
    Ok(Some(Wtf16String::from_units(&units)))
}
