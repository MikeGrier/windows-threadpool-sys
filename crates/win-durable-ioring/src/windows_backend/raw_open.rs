// Copyright (c) 2026 Mike Grier
//! The open a raw volume and a raw disk share (DI-13.11, DI-D-61, DI-D-62).
//!
//! Both are opened by a path the system takes as a device, with the data access and buffering mode
//! the type names and a share mode the caller chooses, and nothing else. The call is made here, once,
//! so what a device open asks of the kernel is one decision and not two that can drift, and so that
//! it can be tried on a file, which needs no administrator: the open is the same call whatever the
//! path names, and only what the kernel makes of the device is the device's.

use std::fs::{File, OpenOptions};
use std::io;
use std::os::windows::fs::OpenOptionsExt;
use std::path::Path;

use durable_ioring::DataAccess;
use wtf_string::Wtf16String;

use super::file_rights::FileRights;
use super::opened_file::{Buffering, data_access, data_rights, refine_sharing};
use super::share_mode::ShareMode;

#[cfg(test)]
mod tests;

/// What a device open asks of `CreateFileW`: the three parameters a raw volume or disk chooses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct RawOpen {
    /// `dwDesiredAccess`.
    pub access: u32,
    /// `dwFlagsAndAttributes`.
    pub flags: u32,
    /// `dwShareMode`.
    pub share: u32,
}

/// What a device opened with data access `A` in buffering mode `B` and sharing `share` asks for.
pub(super) fn raw_open_of<B: Buffering, A: DataAccess>(share: ShareMode) -> RawOpen {
    RawOpen {
        access: data_rights::<A>(),
        flags: B::FLAGS,
        share: share.bits(),
    }
}

/// Open the device or file at `path` as `raw_open_of` says. A conflict with how the device is
/// already open is a [`SharingViolation`](super::SharingViolation), as it is for a file.
pub(super) fn open<B: Buffering, A: DataAccess>(
    path: impl AsRef<Path>,
    share: ShareMode,
) -> io::Result<File> {
    let path = path.as_ref();
    let asked = raw_open_of::<B, A>(share);
    OpenOptions::new()
        .access_mode(asked.access)
        .share_mode(asked.share)
        .custom_flags(asked.flags)
        .open(path)
        .map_err(|error| {
            let given = Wtf16String::from_os_str(path.as_os_str());
            refine_sharing(error, &given, data_access::<A>(), FileRights::NONE, share)
        })
}
