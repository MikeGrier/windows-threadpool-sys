// Copyright (c) 2026 Mike Grier
//! [`FileRights`]: the access rights an open asks for, as a typed set (DI-13.3, DI-D-64).
//!
//! `CreateFileW` takes a `dwDesiredAccess` mask, and this is that mask as a type. Its members are the
//! documented file access rights and the generic rights, each a named constant, and a set is built by
//! naming them and joining them. There is no constructor from a raw number, so the only masks that
//! exist are ones this module's table lists, which is the whole of how an option that DI-D-64 excludes
//! stays excluded: append-only access has no member of its own, and nothing here can remove
//! `FILE_WRITE_DATA` from a set that carries `FILE_APPEND_DATA`, so a handle that appends and cannot
//! write at an offset cannot be asked for.

use std::fmt;
use std::ops::{BitOr, BitOrAssign};

use windows_sys::Win32::Foundation::{GENERIC_ALL, GENERIC_EXECUTE, GENERIC_READ, GENERIC_WRITE};
use windows_sys::Win32::Storage::FileSystem::{
    DELETE, FILE_ALL_ACCESS, FILE_EXECUTE, FILE_READ_ATTRIBUTES, FILE_READ_DATA, FILE_READ_EA,
    FILE_WRITE_ATTRIBUTES, FILE_WRITE_DATA, FILE_WRITE_EA, READ_CONTROL, SYNCHRONIZE, WRITE_DAC,
    WRITE_OWNER,
};
use windows_sys::Win32::System::SystemServices::ACCESS_SYSTEM_SECURITY;

#[cfg(test)]
mod tests;

/// A set of file access rights, the `dwDesiredAccess` of an open.
///
/// Each member is one right the documentation lists for a file, named as `CreateFileW` names it
/// without its prefix: [`READ_DATA`](Self::READ_DATA) is `FILE_READ_DATA`, and the standard and
/// generic rights keep their names. A set is made by joining them with `|`:
///
/// ```
/// use win_durable_ioring::windows_backend::FileRights;
///
/// let rights = FileRights::READ_DATA | FileRights::WRITE_DATA | FileRights::READ_ATTRIBUTES;
///
/// assert!(rights.contains(FileRights::READ_DATA));
/// assert!(!rights.contains(FileRights::DELETE));
/// assert!(FileRights::NONE.is_empty());
/// ```
///
/// **Generic rights are the kernel's to expand.** [`GENERIC_READ`](Self::GENERIC_READ) and the other
/// generic rights are passed as the generic bit, and `CreateFileW` maps it to the file's specific
/// rights when it opens, so [`contains`](Self::contains) compares the bits as they were named and a set
/// holding only `GENERIC_READ` does not contain `READ_DATA`. A file target needs a data right, and a
/// generic right that maps to one counts.
///
/// **Append-only access is not offered**, by construction: there is no member for
/// `FILE_APPEND_DATA` and no way to take a right out of a set, so every set that carries the append bit
/// carries `FILE_WRITE_DATA` with it. A write through a handle opened for appending only lands at the end
/// of the file whatever offset it names, and the core addresses every write by offset.
///
/// The set has no constructor from a raw mask. A right Win32 defines and this table does not list
/// cannot be expressed until the table gains it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct FileRights(u32);

/// Names the rights once, so the constants, the names `Debug` prints and the tests that walk every
/// right are one list and not three that can disagree.
macro_rules! rights {
    (
        singles { $( $(#[$sdoc:meta])* $sname:ident = $svalue:expr; )* }
        composites { $( $(#[$cdoc:meta])* $cname:ident = $cvalue:expr; )* }
    ) => {
        impl FileRights {
            $( $(#[$sdoc])* pub const $sname: Self = Self($svalue); )*
            $( $(#[$cdoc])* pub const $cname: Self = Self($cvalue); )*
        }

        /// Every named right that is a single right, in the order they are declared.
        const SINGLES: &[(&str, FileRights)] = &[ $( (stringify!($sname), FileRights::$sname), )* ];

        /// Every named right that is a set of others.
        const COMPOSITES: &[(&str, FileRights)] = &[ $( (stringify!($cname), FileRights::$cname), )* ];
    };
}

rights! {
    singles {
        /// `FILE_READ_DATA`: read the file's data.
        READ_DATA = FILE_READ_DATA;
        /// `FILE_WRITE_DATA`: write the file's data at an offset.
        WRITE_DATA = FILE_WRITE_DATA;
        /// `FILE_READ_ATTRIBUTES`: read the file's attributes.
        READ_ATTRIBUTES = FILE_READ_ATTRIBUTES;
        /// `FILE_WRITE_ATTRIBUTES`: write the file's attributes.
        WRITE_ATTRIBUTES = FILE_WRITE_ATTRIBUTES;
        /// `FILE_READ_EA`: read the file's extended attributes.
        READ_EA = FILE_READ_EA;
        /// `FILE_WRITE_EA`: write the file's extended attributes.
        WRITE_EA = FILE_WRITE_EA;
        /// `FILE_EXECUTE`: execute the file, for a native code file.
        EXECUTE = FILE_EXECUTE;
        /// `DELETE`: delete or rename the file.
        DELETE = DELETE;
        /// `READ_CONTROL`: read the file's security descriptor, without its system ACL.
        READ_CONTROL = READ_CONTROL;
        /// `WRITE_DAC`: change the file's discretionary ACL.
        WRITE_DAC = WRITE_DAC;
        /// `WRITE_OWNER`: change the file's owner.
        WRITE_OWNER = WRITE_OWNER;
        /// `SYNCHRONIZE`: wait on the handle.
        SYNCHRONIZE = SYNCHRONIZE;
        /// `ACCESS_SYSTEM_SECURITY`: read and change the file's system ACL. A caller the kernel does
        /// not allow it gets the kernel's error from the open.
        ACCESS_SYSTEM_SECURITY = ACCESS_SYSTEM_SECURITY;
        /// `GENERIC_READ`, which `CreateFileW` maps to the file's read rights.
        GENERIC_READ = GENERIC_READ;
        /// `GENERIC_WRITE`, which `CreateFileW` maps to the file's write rights, among them
        /// `FILE_WRITE_DATA`.
        GENERIC_WRITE = GENERIC_WRITE;
        /// `GENERIC_EXECUTE`, which `CreateFileW` maps to the file's execute rights. It maps to no
        /// data right, so a set holding only this one is not enough for a file target.
        GENERIC_EXECUTE = GENERIC_EXECUTE;
        /// `GENERIC_ALL`, which `CreateFileW` maps to every right of a file.
        GENERIC_ALL = GENERIC_ALL;
    }
    composites {
        /// `FILE_ALL_ACCESS`: every right of a file, which includes the append and directory bits
        /// that have no member of their own.
        ///
        /// The append bit is one the kernel documents as incompatible with an unbuffered open, and
        /// an [`Unbuffered`](super::Unbuffered) open of a set holding this one was measured to be
        /// refused with `ERROR_INVALID_PARAMETER`. A [`Cached`](super::Cached) open takes it, and so
        /// does an unbuffered open of [`GENERIC_ALL`](Self::GENERIC_ALL).
        ALL_ACCESS = FILE_ALL_ACCESS;
    }
}

impl FileRights {
    /// No rights. A file target cannot be opened with only this: it needs a data right.
    pub const NONE: Self = Self(0);

    /// The union of two sets.
    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// Whether every right of `other` is in this set, comparing the bits as they were named.
    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// Whether the set holds no right.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// The mask `CreateFileW` is given.
    pub(super) const fn bits(self) -> u32 {
        self.0
    }

    /// Whether this set grants reading the file's data, directly or through a generic right the kernel
    /// maps to it.
    pub(super) const fn grants_read_data(self) -> bool {
        self.0 & (FILE_READ_DATA | GENERIC_READ | GENERIC_ALL) != 0
    }

    /// Whether this set grants writing the file's data, directly or through a generic right the kernel
    /// maps to it.
    pub(super) const fn grants_write_data(self) -> bool {
        self.0 & (FILE_WRITE_DATA | GENERIC_WRITE | GENERIC_ALL) != 0
    }

    /// Whether a file target can be made from a file opened with these rights: it has to read or write.
    pub(super) const fn grants_a_data_right(self) -> bool {
        self.grants_read_data() || self.grants_write_data()
    }
}

impl BitOr for FileRights {
    type Output = Self;

    fn bitor(self, other: Self) -> Self {
        self.union(other)
    }
}

impl BitOrAssign for FileRights {
    fn bitor_assign(&mut self, other: Self) {
        *self = self.union(other);
    }
}

impl fmt::Debug for FileRights {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("FileRights(")?;
        if self.is_empty() {
            f.write_str("none")?;
        } else if let Some((name, _)) = COMPOSITES.iter().find(|(_, set)| set == self) {
            f.write_str(name)?;
        } else {
            let mut covered = 0;
            let mut first = true;
            for (name, right) in SINGLES.iter().filter(|(_, right)| self.contains(*right)) {
                if !first {
                    f.write_str(" | ")?;
                }
                first = false;
                f.write_str(name)?;
                covered |= right.0;
            }
            // Bits the singles do not name, which only a composite brings.
            let rest = self.0 & !covered;
            if rest != 0 {
                if !first {
                    f.write_str(" | ")?;
                }
                write!(f, "{rest:#010x}")?;
            }
        }
        f.write_str(")")
    }
}
