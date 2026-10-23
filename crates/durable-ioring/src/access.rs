// Copyright (c) 2026 Mike Grier
//! What a target can do with the data it addresses (DI-13.9, DI-D-66).
//!
//! Two axes decide it: whether the data can be read and whether it can be written. They give four
//! markers, [`NoData`], [`ReadOnly`], [`WriteOnly`] and [`ReadWrite`], which a backend's handles carry
//! as a type parameter where they are opened (`win-durable-ioring`'s `OpenedFile<B, A>` is one), and
//! which the traits here sort: [`HasData`] for the three that can read or write, [`CanRead`] and
//! [`CanWrite`] for those that can do each.
//!
//! A target is one type per backend and is held by a ring under a number, so it cannot carry the
//! marker as a type parameter without letting a ring hold targets of one access only. It states its
//! access instead, as a value of [`Access`], read once when it is given to an instance, as it states
//! its warranty and its geometry. A [`HasData`] marker names the value that stands for it, which is
//! how a target built from a typed handle states the access that handle was opened with.
//!
//! What a marker asks of a platform is the backend's: `win-durable-ioring` turns [`DataAccess::READ`]
//! and [`DataAccess::WRITE`] into the rights it opens with.

use core::fmt;

mod sealed {
    pub trait Sealed {}
}

/// What a target can do with its data, stated by the target. A target has data to address, so there
/// is no value for neither: a handle that can do neither is not a target
/// ([`NoData`] is not [`HasData`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Access {
    /// It can be read and not written.
    ReadOnly,
    /// It can be written and not read.
    WriteOnly,
    /// It can be read and written.
    ReadWrite,
}

impl Access {
    /// Whether the data can be read.
    #[must_use]
    pub const fn can_read(self) -> bool {
        matches!(self, Self::ReadOnly | Self::ReadWrite)
    }

    /// Whether the data can be written.
    #[must_use]
    pub const fn can_write(self) -> bool {
        matches!(self, Self::WriteOnly | Self::ReadWrite)
    }

    /// Whether everything `other` can do, this can: what a target must be able to do for a key
    /// that declares `other` to be registered under it.
    #[must_use]
    pub const fn covers(self, other: Self) -> bool {
        (self.can_read() || !other.can_read()) && (self.can_write() || !other.can_write())
    }
}

impl fmt::Display for Access {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::ReadOnly => "read only",
            Self::WriteOnly => "write only",
            Self::ReadWrite => "read and write",
        })
    }
}

/// The access a handle was opened with, as a type. Sealed: there are four markers, one for each
/// combination of reading and writing.
pub trait DataAccess: sealed::Sealed + Copy + fmt::Debug + Send + Sync + 'static {
    /// Whether this access can read the data.
    const READ: bool;

    /// Whether this access can write the data.
    const WRITE: bool;

    /// This access with reading added.
    type WithRead: DataAccess;

    /// This access with writing added.
    type WithWrite: DataAccess;
}

/// A [`DataAccess`] that can read or write, so that a target can be made from it, and the [`Access`]
/// a target made from it states. [`NoData`] is not one:
///
/// ```compile_fail,E0277
/// use durable_ioring::{HasData, NoData};
///
/// fn needs_data<A: HasData>() {}
/// needs_data::<NoData>();
/// ```
///
/// ```
/// use durable_ioring::{Access, HasData, ReadOnly, ReadWrite, WriteOnly};
///
/// assert_eq!(ReadOnly::ACCESS, Access::ReadOnly);
/// assert_eq!(WriteOnly::ACCESS, Access::WriteOnly);
/// assert_eq!(ReadWrite::ACCESS, Access::ReadWrite);
/// ```
pub trait HasData: DataAccess {
    /// What a target made from a handle with this access states.
    const ACCESS: Access;
}

/// A [`DataAccess`] that can read. [`WriteOnly`] and [`NoData`] are not one:
///
/// ```compile_fail,E0277
/// use durable_ioring::{CanRead, WriteOnly};
///
/// fn needs_read<A: CanRead>() {}
/// needs_read::<WriteOnly>();
/// ```
///
/// ```compile_fail,E0277
/// use durable_ioring::{CanRead, NoData};
///
/// fn needs_read<A: CanRead>() {}
/// needs_read::<NoData>();
/// ```
pub trait CanRead: HasData {}

/// A [`DataAccess`] that can write. [`ReadOnly`] and [`NoData`] are not one:
///
/// ```compile_fail,E0277
/// use durable_ioring::{CanWrite, ReadOnly};
///
/// fn needs_write<A: CanWrite>() {}
/// needs_write::<ReadOnly>();
/// ```
///
/// ```compile_fail,E0277
/// use durable_ioring::{CanWrite, NoData};
///
/// fn needs_write<A: CanWrite>() {}
/// needs_write::<NoData>();
/// ```
pub trait CanWrite: HasData {}

/// No data right: a handle that can address a file or device without reading or writing it, which is
/// enough to query it. It cannot be made into a target.
#[derive(Clone, Copy, Debug)]
pub struct NoData;

/// The right to read the data and not to write it.
#[derive(Clone, Copy, Debug)]
pub struct ReadOnly;

/// The right to write the data and not to read it.
#[derive(Clone, Copy, Debug)]
pub struct WriteOnly;

/// The rights to read and to write the data.
#[derive(Clone, Copy, Debug)]
pub struct ReadWrite;

impl sealed::Sealed for NoData {}
impl sealed::Sealed for ReadOnly {}
impl sealed::Sealed for WriteOnly {}
impl sealed::Sealed for ReadWrite {}

impl DataAccess for NoData {
    const READ: bool = false;
    const WRITE: bool = false;
    type WithRead = ReadOnly;
    type WithWrite = WriteOnly;
}

impl DataAccess for ReadOnly {
    const READ: bool = true;
    const WRITE: bool = false;
    type WithRead = ReadOnly;
    type WithWrite = ReadWrite;
}

impl DataAccess for WriteOnly {
    const READ: bool = false;
    const WRITE: bool = true;
    type WithRead = ReadWrite;
    type WithWrite = WriteOnly;
}

impl DataAccess for ReadWrite {
    const READ: bool = true;
    const WRITE: bool = true;
    type WithRead = ReadWrite;
    type WithWrite = ReadWrite;
}

impl HasData for ReadOnly {
    const ACCESS: Access = Access::ReadOnly;
}

impl HasData for WriteOnly {
    const ACCESS: Access = Access::WriteOnly;
}

impl HasData for ReadWrite {
    const ACCESS: Access = Access::ReadWrite;
}

impl CanRead for ReadOnly {}
impl CanRead for ReadWrite {}

impl CanWrite for WriteOnly {}
impl CanWrite for ReadWrite {}

#[cfg(test)]
mod tests;
