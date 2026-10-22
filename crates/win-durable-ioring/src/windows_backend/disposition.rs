// Copyright (c) 2026 Mike Grier
//! [`Disposition`]: what an open does about a file that is there and a file that is not, as
//! `CreateFileW`'s `dwCreationDisposition` states it (DI-13.7.3, DI-D-64).
//!
//! `CreateFileW` has five dispositions. Three are offered here. `CREATE_ALWAYS` and
//! `TRUNCATE_EXISTING` are the two that empty a file that exists, and they are not offered in the
//! first release (the engineer's ruling of 2026-10-16, recorded with its measurements in DI-D-64);
//! the enum is `#[non_exhaustive]` so that offering them later is not a breaking change. A caller
//! that wants an existing file emptied does it through the handle the open lends, with
//! `File::set_len(0)`.

use windows_sys::Win32::Storage::FileSystem::{CREATE_NEW, OPEN_ALWAYS, OPEN_EXISTING};

#[cfg(test)]
mod tests;

/// The disposition of an open: what it does when the file exists and when it does not.
///
/// None of the three changes the contents of a file that exists, so no data access is gated by the
/// choice.
///
/// | Variant | `dwCreationDisposition` | File exists | File does not exist |
/// |---|---|---|---|
/// | [`CreateNew`](Self::CreateNew) | `CREATE_NEW` | fails, `ERROR_FILE_EXISTS` | created |
/// | [`OpenExisting`](Self::OpenExisting) | `OPEN_EXISTING` | opened | fails, `ERROR_FILE_NOT_FOUND` |
/// | [`OpenAlways`](Self::OpenAlways) | `OPEN_ALWAYS` | opened | created |
///
/// A file that an open creates is created empty.
///
/// There is deliberately no `Default`: the options start at [`OpenExisting`](Self::OpenExisting),
/// which creates nothing, and a disposition is stated where one is wanted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Disposition {
    /// `CREATE_NEW`: create the file, and fail if it exists.
    CreateNew,
    /// `OPEN_EXISTING`: open the file, and fail if it does not exist.
    OpenExisting,
    /// `OPEN_ALWAYS`: open the file if it exists, and create it if it does not.
    OpenAlways,
}

impl Disposition {
    /// The value this is in `dwCreationDisposition`.
    pub(super) const fn bits(self) -> u32 {
        match self {
            Self::CreateNew => CREATE_NEW,
            Self::OpenExisting => OPEN_EXISTING,
            Self::OpenAlways => OPEN_ALWAYS,
        }
    }
}
