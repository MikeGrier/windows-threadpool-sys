// Copyright (c) 2026 Mike Grier
//! The pipe name every call that takes one passes: a NUL-terminated UTF-16 string.

use std::ffi::OsStr;
use std::io;
use std::os::windows::ffi::OsStrExt;

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
