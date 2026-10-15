// Copyright (c) Mike Grier.

//! The `FSCTL_QUERY_VOLUME_NUMA_INFO` entry (NR-2.1).
//!
//! A storage-locality entry, added after round one for durable-ioring's flush domains: the NUMA
//! node the **volume** a file or directory is on resides on, through `DeviceIoControl`.
//!
//! # What the answer is, and is not
//!
//! The documented meaning is the node of the *volume that contains* the file or directory. It
//! says nothing of where a file's extents are, so it is a statement about a volume however it is
//! reached. A volume that spans several devices still gets one answer, so the answer is not
//! evidence about each of them; that is what the per-disk entries of NR-2.2 to NR-2.4 are for.
//!
//! # Whether the device has an answer
//!
//! The documentation says the control "finds the current NUMA node index for a volume" and does
//! not say what a volume that advertises no proximity domain returns, so the entry does not guess:
//! a failure is the device's own code, unaltered, and a success carries the value the device
//! wrote, unaltered. Neither is interpreted.
//!
//! # A control, so the one helper
//!
//! The call goes through the crate's one `DeviceIoControl` helper (D-20), which is the same call
//! on an overlapped handle and on a plain one.

use std::fmt;

use windows_sys::Win32::System::Ioctl::FSCTL_QUERY_VOLUME_NUMA_INFO;

use crate::device_control::device_control;
use crate::handle::{CapturedHandle, HandleCaptureError};
use crate::outcome::Win32Error;

#[cfg(test)]
mod tests;

/// The size of `FSCTL_QUERY_VOLUME_NUMA_INFO_OUTPUT`, which is a single `ULONG`, `NumaNode`.
///
/// The structure is declared in `ntifs.h` and, in `windows-sys`, only under its driver-kit
/// namespace, so this crate states its one field here instead of taking that feature for a `u32`.
/// The documentation says an output buffer that is too small fails with
/// `ERROR_INSUFFICIENT_BUFFER`, so a structure that grew would be reported and not truncated.
const OUTPUT_SIZE: usize = size_of::<u32>();

/// What a volume reported as the NUMA node it resides on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct VolumeNumaInfo {
    node: u32,
}

impl VolumeNumaInfo {
    /// The node, as the device wrote it.
    ///
    /// Carried unaltered, so a value that is not a node of this machine reaches the caller as it
    /// was: whether such a value means "no node" is for the caller to decide, since the
    /// documentation does not say.
    #[must_use]
    pub fn node(&self) -> u32 {
        self.node
    }
}

impl fmt::Display for VolumeNumaInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "NUMA node {}", self.node)
    }
}

/// Why a volume's NUMA node could not be read.
#[derive(Debug)]
#[non_exhaustive]
pub enum NumaInfoError {
    /// Windows refused the call, with the raw code unaltered.
    ///
    /// This is also the shape of a device that advertises no proximity domain, whatever code it
    /// answers with: the entry does not decide which codes mean that.
    Win32(Win32Error),
    /// The call succeeded but reported a size other than the one `ULONG` the structure is.
    ///
    /// Reported and not read, because a value taken from a buffer the device did not fill is
    /// worse than no answer: it would read as a node.
    Unexpected {
        /// How many bytes the call said it wrote.
        returned: u32,
    },
}

impl fmt::Display for NumaInfoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Win32(error) => write!(f, "FSCTL_QUERY_VOLUME_NUMA_INFO: {error}"),
            Self::Unexpected { returned } => write!(
                f,
                "FSCTL_QUERY_VOLUME_NUMA_INFO: the call wrote {returned} bytes, not the {OUTPUT_SIZE} \
                 its output structure is"
            ),
        }
    }
}

impl std::error::Error for NumaInfoError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Win32(error) => Some(error),
            Self::Unexpected { .. } => None,
        }
    }
}

impl From<Win32Error> for NumaInfoError {
    fn from(error: Win32Error) -> Self {
        Self::Win32(error)
    }
}

/// An owned, marshalable parameter set for `DeviceIoControl` with
/// `FSCTL_QUERY_VOLUME_NUMA_INFO`.
///
/// # Example
///
/// The handle names a file or a directory on the volume, and the volume is what gets reported. A
/// device that advertises no proximity domain answers with its own error, which is not a failure
/// of the request:
///
/// ```
/// use std::fs;
/// use std::os::windows::io::AsHandle;
///
/// use windows_namespace_request_sys::volume_numa::{NumaInfoError, QueryVolumeNumaInfo};
/// use windows_namespace_request_sys::CapturedHandle;
///
/// let path = std::env::temp_dir().join(format!("wnrs-numa-{}.tmp", std::process::id()));
/// fs::write(&path, b"example")?;
/// let file = fs::File::open(&path)?;
///
/// let request = QueryVolumeNumaInfo::new(CapturedHandle::capture(file.as_handle())?);
/// match request.perform() {
///     // The node the volume resides on, as the device wrote it.
///     Ok(info) => println!("{info}"),
///     // The device's own code: no answer, and nothing to be done about it here.
///     Err(NumaInfoError::Win32(error)) => println!("{error}"),
///     Err(other) => panic!("{other}"),
/// }
/// # drop(file);
/// # fs::remove_file(&path)?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug)]
#[must_use = "an unperformed request queries nothing"]
pub struct QueryVolumeNumaInfo {
    handle: CapturedHandle,
}

impl QueryVolumeNumaInfo {
    /// Begins a request against `handle`.
    ///
    /// The handle names a file or directory on the volume, or the volume itself; the volume is
    /// what gets reported.
    pub fn new(handle: CapturedHandle) -> Self {
        Self { handle }
    }

    /// The owned duplicate of the handle being queried.
    pub fn handle(&self) -> &CapturedHandle {
        &self.handle
    }

    /// Copies the request, duplicating the handle.
    ///
    /// # Errors
    ///
    /// Returns the handle-capture failure when the handle cannot be duplicated.
    pub fn try_clone(&self) -> Result<Self, HandleCaptureError> {
        Ok(Self {
            handle: self.handle.try_clone()?,
        })
    }

    /// Performs the query on the calling thread.
    ///
    /// The same call whether the handle was opened overlapped or not, and a handle that belongs
    /// to a completion port gets no packet for it.
    ///
    /// # Errors
    ///
    /// [`NumaInfoError::Win32`] with the raw code, unaltered, or [`NumaInfoError::Unexpected`] if
    /// the call succeeded and reported a size other than the structure's.
    pub fn perform(&self) -> Result<VolumeNumaInfo, NumaInfoError> {
        let mut output = [0_u8; OUTPUT_SIZE];
        let returned =
            device_control(self.handle.raw(), FSCTL_QUERY_VOLUME_NUMA_INFO, &mut output)?;
        decode(returned, output)
    }
}

/// Reads the structure out of what the call wrote, if it wrote all of it and only it.
fn decode(returned: u32, output: [u8; OUTPUT_SIZE]) -> Result<VolumeNumaInfo, NumaInfoError> {
    if usize::try_from(returned) != Ok(OUTPUT_SIZE) {
        return Err(NumaInfoError::Unexpected { returned });
    }
    Ok(VolumeNumaInfo {
        node: u32::from_ne_bytes(output),
    })
}

impl crate::request::Request for QueryVolumeNumaInfo {
    type Error = NumaInfoError;
    type Output = VolumeNumaInfo;

    fn perform(&self) -> Result<VolumeNumaInfo, NumaInfoError> {
        Self::perform(self)
    }
}
