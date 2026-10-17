// Copyright (c) 2026 Mike Grier
//! The data access a handle was opened with, as a type (DI-13.8, DI-D-65).
//!
//! Two axes decide what this crate's own API may do with a handle: whether it can read the data and
//! whether it can write it. They give four markers, [`NoData`], [`ReadOnly`], [`WriteOnly`] and
//! [`ReadWrite`], which [`OpenedFile`](super::OpenedFile), [`OpenedDisk`](super::OpenedDisk) and
//! [`OpenedVolume`](super::OpenedVolume) carry as their last type parameter, and which
//! [`FileOptions`](super::FileOptions) moves along as it is told to read and to write. A
//! [`Win32FileTarget`](super::Win32FileTarget) is made only from a handle with a data right
//! ([`HasData`]), so a handle that can do neither is refused when the program is compiled.
//!
//! The other rights an open can ask for, among them the attribute, delete and security rights, gate
//! only calls a consumer makes on the handle it is lent, and stay in
//! [`FileRights`](super::FileRights), where the kernel judges them.

use std::fmt;

use windows_sys::Win32::Storage::FileSystem::{FILE_READ_DATA, FILE_WRITE_DATA};

mod sealed {
    pub trait Sealed {}
}

/// What a handle can do with the data it addresses, known when it was opened. Sealed: there are four
/// markers, one for each combination of reading and writing, and this crate states what each asks of
/// `CreateFileW`.
pub trait DataAccess: sealed::Sealed + Copy + fmt::Debug + Send + Sync + 'static {
    /// The `dwDesiredAccess` bits this access asks for: `FILE_READ_DATA` and `FILE_WRITE_DATA` as it
    /// reads and writes, and no other. The append bit is never among them, so a handle that appends
    /// and cannot write at an offset cannot be asked for.
    const RIGHTS: u32;

    /// This access with reading added.
    type WithRead: DataAccess;

    /// This access with writing added.
    type WithWrite: DataAccess;
}

/// A [`DataAccess`] that can read or write, so that a target can be made from it. [`NoData`] is not
/// one:
///
/// ```compile_fail,E0277
/// use win_durable_ioring::windows_backend::{HasData, NoData};
///
/// fn needs_data<A: HasData>() {}
/// needs_data::<NoData>();
/// ```
///
/// ```
/// use win_durable_ioring::windows_backend::{HasData, ReadOnly, ReadWrite, WriteOnly};
///
/// fn needs_data<A: HasData>() {}
/// needs_data::<ReadOnly>();
/// needs_data::<WriteOnly>();
/// needs_data::<ReadWrite>();
/// ```
pub trait HasData: DataAccess {}

/// A [`DataAccess`] that can read. [`WriteOnly`] and [`NoData`] are not one:
///
/// ```compile_fail,E0277
/// use win_durable_ioring::windows_backend::{CanRead, WriteOnly};
///
/// fn needs_read<A: CanRead>() {}
/// needs_read::<WriteOnly>();
/// ```
///
/// ```compile_fail,E0277
/// use win_durable_ioring::windows_backend::{CanRead, NoData};
///
/// fn needs_read<A: CanRead>() {}
/// needs_read::<NoData>();
/// ```
pub trait CanRead: HasData {}

/// A [`DataAccess`] that can write. [`ReadOnly`] and [`NoData`] are not one:
///
/// ```compile_fail,E0277
/// use win_durable_ioring::windows_backend::{CanWrite, ReadOnly};
///
/// fn needs_write<A: CanWrite>() {}
/// needs_write::<ReadOnly>();
/// ```
///
/// ```compile_fail,E0277
/// use win_durable_ioring::windows_backend::{CanWrite, NoData};
///
/// fn needs_write<A: CanWrite>() {}
/// needs_write::<NoData>();
/// ```
pub trait CanWrite: HasData {}

/// No data right: a handle that can address the file or device without reading or writing it, which
/// is enough to query it. It cannot be made into a target.
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
    const RIGHTS: u32 = 0;
    type WithRead = ReadOnly;
    type WithWrite = WriteOnly;
}

impl DataAccess for ReadOnly {
    const RIGHTS: u32 = FILE_READ_DATA;
    type WithRead = ReadOnly;
    type WithWrite = ReadWrite;
}

impl DataAccess for WriteOnly {
    const RIGHTS: u32 = FILE_WRITE_DATA;
    type WithRead = ReadWrite;
    type WithWrite = WriteOnly;
}

impl DataAccess for ReadWrite {
    const RIGHTS: u32 = FILE_READ_DATA | FILE_WRITE_DATA;
    type WithRead = ReadWrite;
    type WithWrite = ReadWrite;
}

impl HasData for ReadOnly {}
impl HasData for WriteOnly {}
impl HasData for ReadWrite {}

impl CanRead for ReadOnly {}
impl CanRead for ReadWrite {}

impl CanWrite for WriteOnly {}
impl CanWrite for ReadWrite {}

#[cfg(test)]
mod tests;
