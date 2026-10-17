// Copyright (c) 2026 Mike Grier
//! The open itself, made through `windows-namespace-request-sys`'s `OpenFile`, so this crate has
//! no `unsafe` of its own for it (DI-13.2).
//!
//! That crate carries every parameter of `CreateFileW` and decides none of them, so what an open
//! means is decided here: [`Parameters`] is what [`FileOptions`](super::FileOptions) asks for, and
//! [`open`] makes the call. The path is handed to the request as the caller gave it, a
//! [`Wtf16String`] (DI-13.7.1): nothing here lengthens, shortens or refuses it, so a path longer than
//! `MAX_PATH` without a verbatim prefix and an empty one are what Win32 makes of them. A file that
//! is both created and truncated is still opened with `OPEN_ALWAYS` and then emptied, as `std`
//! does, until DI-13.7.3 replaces the booleans with Win32's own dispositions.

use std::fs::File;
use std::io;
use std::os::windows::io::OwnedHandle;

use windows_namespace_request_sys::PathCaptureError;
use windows_namespace_request_sys::open::OpenFile;
use wtf_string::Wtf16String;

#[cfg(test)]
mod tests;

/// What an open asks of `CreateFileW`, already checked for the combinations that cannot mean
/// anything.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Parameters {
    /// `dwDesiredAccess`.
    pub access: u32,
    /// `dwCreationDisposition`.
    pub disposition: u32,
    /// `dwFlagsAndAttributes`.
    pub flags: u32,
    /// `dwShareMode`.
    pub share: u32,
    /// Whether to empty the file once it is open, for a file that is created and truncated.
    pub truncate_after: bool,
}

/// Open `path` as `parameters` says. A path with an interior NUL is refused by the request, as
/// invalid input, before anything is touched.
pub(super) fn open(path: Wtf16String, parameters: Parameters) -> io::Result<OwnedHandle> {
    let request = OpenFile::new(path)
        .map_err(path_error)?
        .with_desired_access(parameters.access)
        .with_share_mode(parameters.share)
        .with_creation_disposition(parameters.disposition)
        .with_flags_and_attributes(parameters.flags);
    let handle = request
        .perform()
        .map_err(|error| io::Error::from_raw_os_error(raw_code(error.code())))?;
    if !parameters.truncate_after {
        return Ok(handle);
    }
    let file = File::from(handle);
    file.set_len(0)?;
    Ok(OwnedHandle::from(file))
}

/// A path the request refused, as the error an open reports: the Windows code if the refusal came
/// from a call, and invalid input if the path itself could not mean a file.
fn path_error(error: PathCaptureError) -> io::Error {
    match error {
        PathCaptureError::CurrentDirectory(code) => {
            io::Error::from_raw_os_error(raw_code(code.code()))
        }
        other => io::Error::new(io::ErrorKind::InvalidInput, other),
    }
}

/// A Win32 error code as the signed value `io::Error` holds, bit for bit.
fn raw_code(code: u32) -> i32 {
    i32::from_ne_bytes(code.to_ne_bytes())
}
