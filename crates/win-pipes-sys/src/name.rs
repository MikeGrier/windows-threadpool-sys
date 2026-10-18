// Copyright (c) 2026 Mike Grier
//! The wide strings the calls take and give: a pipe name going in, and a NUL-terminated string coming
//! out of a buffer the caller sized.

use std::ffi::{OsStr, OsString};
use std::io;
use std::os::windows::ffi::{OsStrExt, OsStringExt};

#[cfg(test)]
mod tests;

/// `name` as the NUL-terminated wide string the calls take. The one argument this crate cannot pass as
/// given is a name with an interior NUL, which a C string cannot hold, so that is
/// [`io::ErrorKind::InvalidInput`] and the call is not made.
pub(crate) fn wide_name(name: &OsStr) -> io::Result<Vec<u16>> {
    let mut wide: Vec<u16> = name.encode_wide().collect();
    if wide.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "a pipe name with an interior NUL",
        ));
    }
    wide.push(0);
    Ok(wide)
}

/// The string in `units` up to its first NUL, or all of `units` if it has none: what a call that
/// writes a NUL-terminated string into a buffer the caller sized has given. A unit that is not valid
/// UTF-16 is kept.
pub(crate) fn from_wide_until_nul(units: &[u16]) -> OsString {
    let end = units
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(units.len());
    OsString::from_wide(&units[..end])
}
