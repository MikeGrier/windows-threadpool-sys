// Copyright (c) 2026 Mike Grier
//! [`FileRights`]: the rights an open asks for beyond reading and writing the data, as a typed set
//! (DI-13.3, DI-13.8, DI-D-64, DI-D-65).
//!
//! `CreateFileW` takes a `dwDesiredAccess` mask. The two rights that decide what this crate's own API
//! may do with a handle, `FILE_READ_DATA` and `FILE_WRITE_DATA`, are the data access type
//! ([`DataAccess`](super::DataAccess)), and this set holds the rest: the attribute, extended
//! attribute, execute, delete and security rights, which gate only calls a consumer makes on the
//! handle it is lent. Each member is a named constant, and a set is built by naming them and joining
//! them. There is no constructor from a raw number, so the only masks that exist are ones this
//! module's table lists, which is the whole of how an option that DI-D-64 excludes stays excluded:
//! there is no member for `FILE_APPEND_DATA`, and no generic right, whose expansion the kernel makes,
//! so a handle that appends and cannot write at an offset cannot be asked for.

use std::fmt;
use std::ops::{BitOr, BitOrAssign};

use windows_sys::Win32::Storage::FileSystem::{
    DELETE, FILE_EXECUTE, FILE_READ_ATTRIBUTES, FILE_READ_EA, FILE_WRITE_ATTRIBUTES, FILE_WRITE_EA,
    READ_CONTROL, SYNCHRONIZE, WRITE_DAC, WRITE_OWNER,
};
use windows_sys::Win32::System::SystemServices::ACCESS_SYSTEM_SECURITY;

#[cfg(test)]
mod tests;

/// A set of file access rights other than the data rights, which are the
/// [`DataAccess`](super::DataAccess) of the handle.
///
/// Each member is one right the documentation lists for a file, named as `CreateFileW` names it
/// without its prefix: [`WRITE_ATTRIBUTES`](Self::WRITE_ATTRIBUTES) is `FILE_WRITE_ATTRIBUTES`, and
/// the standard rights keep their names. A set is made by joining them with `|`:
///
/// ```
/// use win_durable_ioring::windows_backend::FileRights;
///
/// let rights = FileRights::READ_ATTRIBUTES | FileRights::WRITE_ATTRIBUTES;
///
/// assert!(rights.contains(FileRights::WRITE_ATTRIBUTES));
/// assert!(!rights.contains(FileRights::DELETE));
/// assert!(FileRights::NONE.is_empty());
/// ```
///
/// **There is no generic right and no `FILE_ALL_ACCESS`.** A generic right mixes the data rights, which
/// are a type, with others, and the kernel expands it when it opens, so a set could not say what it
/// holds. Ask for the data access with [`FileOptions::read`](super::FileOptions::read) and
/// [`write`](super::FileOptions::write), and for the rest with the members here.
///
/// **Append-only access is not offered**, by construction: there is no member for `FILE_APPEND_DATA`,
/// and no data access asks for it. A write through a handle opened for appending only lands at the end
/// of the file whatever offset it names, and the core addresses every write by offset.
///
/// The set has no constructor from a raw mask. A right Win32 defines and this table does not list
/// cannot be expressed until the table gains it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct FileRights(u32);

/// Names the rights once, so the constants, the names `Debug` prints and the tests that walk every
/// right are one list and not three that can disagree.
macro_rules! rights {
    ( $( $(#[$doc:meta])* $name:ident = $value:expr; )* ) => {
        impl FileRights {
            $( $(#[$doc])* pub const $name: Self = Self($value); )*
        }

        /// Every named right, in the order they are declared.
        const NAMED: &[(&str, FileRights)] = &[ $( (stringify!($name), FileRights::$name), )* ];
    };
}

rights! {
    /// `FILE_READ_ATTRIBUTES`: read the file's attributes.
    READ_ATTRIBUTES = FILE_READ_ATTRIBUTES;
    /// `FILE_WRITE_ATTRIBUTES`: write the file's attributes, among them its times.
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
}

impl FileRights {
    /// No rights beyond the data access.
    pub const NONE: Self = Self(0);

    /// The union of two sets.
    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// Whether every right of `other` is in this set.
    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// Whether the set holds no right.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// The mask this set adds to the data access's, in `dwDesiredAccess`.
    pub(super) const fn bits(self) -> u32 {
        self.0
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
        } else {
            let mut first = true;
            for (name, _) in NAMED.iter().filter(|(_, right)| self.contains(*right)) {
                if !first {
                    f.write_str(" | ")?;
                }
                first = false;
                f.write_str(name)?;
            }
        }
        f.write_str(")")
    }
}
