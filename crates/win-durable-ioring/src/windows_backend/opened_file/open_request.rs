// Copyright (c) 2026 Mike Grier
//! The open itself, made through `windows-namespace-request-sys`'s `OpenFile`, so this crate has
//! no `unsafe` of its own for it (DI-13.2).
//!
//! That crate carries every parameter of `CreateFileW` and decides none of them, so what an open
//! means is decided here: [`Parameters`] is what [`FileOptions`](super::FileOptions) asks for, and
//! [`open`] makes the call. Opening used to go through `std::fs::OpenOptions`, and a caller must
//! see no difference, so the two things std does that `CreateFileW` alone does not are done here
//! too: a path longer than `MAX_PATH` is opened as a verbatim one, and a file that is both created
//! and truncated is opened with `OPEN_ALWAYS` and then emptied, as std does to avoid
//! `CREATE_ALWAYS`'s semantics (rust-lang/rust#115745).

use std::ffi::OsString;
use std::fs::File;
use std::io;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::os::windows::io::OwnedHandle;
use std::path::{Path, PathBuf};

use windows_namespace_request_sys::PathError;
use windows_namespace_request_sys::open::OpenFile;
use windows_namespace_request_sys::prepare;
use windows_sys::Win32::Foundation::MAX_PATH;
use windows_sys::Win32::Storage::FileSystem::{
    FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
};
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
    /// Whether to empty the file once it is open, for a file that is created and truncated.
    pub truncate_after: bool,
}

/// The verbatim prefix of a path, `\\?\`, as UTF-16.
const VERBATIM: [u16; 4] = [b'\\' as u16, b'\\' as u16, b'?' as u16, b'\\' as u16];

/// Open `path` as `parameters` says, shared for reading, writing and deletion as std opens a file.
pub(super) fn open(path: &Path, parameters: Parameters) -> io::Result<OwnedHandle> {
    let path = verbatim_if_long(path)?;
    let prepared = prepare(&Wtf16String::from_os_str(path.as_os_str())).map_err(path_error)?;
    let request = OpenFile::new(prepared)
        .with_desired_access(parameters.access)
        .with_share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
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

/// `path` as it should be handed to the request: itself, unless it or its resolved form is longer
/// than an ordinary path may be, in which case its verbatim form, which has no such limit.
fn verbatim_if_long(path: &Path) -> io::Result<PathBuf> {
    let absolute = std::path::absolute(path)?;
    let longest = MAX_PATH as usize - 1;
    if path.as_os_str().encode_wide().count() <= longest
        && absolute.as_os_str().encode_wide().count() <= longest
    {
        return Ok(path.to_path_buf());
    }
    Ok(verbatim(&absolute))
}

/// The verbatim form of an absolute path: unchanged if it already is one, or names a device, and
/// otherwise with the prefix that turns path parsing off.
fn verbatim(absolute: &Path) -> PathBuf {
    let units: Vec<u16> = absolute.as_os_str().encode_wide().collect();
    let backslash = u16::from(b'\\');
    let question = u16::from(b'?');
    let dot = u16::from(b'.');
    let starts_with_two_backslashes = units.starts_with(&[backslash, backslash]);
    let third = units.get(2).copied();
    if starts_with_two_backslashes && matches!(third, Some(unit) if unit == question || unit == dot)
    {
        return absolute.to_path_buf();
    }
    let mut verbatim = VERBATIM.to_vec();
    if starts_with_two_backslashes {
        verbatim.extend("UNC\\".encode_utf16());
        verbatim.extend_from_slice(&units[2..]);
    } else {
        verbatim.extend_from_slice(&units);
    }
    PathBuf::from(OsString::from_wide(&verbatim))
}

/// A path the request refused, as the error an open reports: the Windows code if the refusal came
/// from a call, and invalid input if the path itself could not mean a file.
fn path_error(error: PathError) -> io::Error {
    match error.raw_os_error() {
        Some(code) => io::Error::from_raw_os_error(code),
        None => io::Error::new(io::ErrorKind::InvalidInput, error),
    }
}

/// A Win32 error code as the signed value `io::Error` holds, bit for bit.
fn raw_code(code: u32) -> i32 {
    i32::from_ne_bytes(code.to_ne_bytes())
}
