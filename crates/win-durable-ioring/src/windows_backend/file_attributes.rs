// Copyright (c) 2026 Mike Grier
//! [`FileAttributes`]: the attributes a file is created with, as a typed set (DI-14.3, DI-D-64).
//!
//! `CreateFileW` takes the attributes of a file it creates in `dwFlagsAndAttributes`. The set here is
//! the six the table keeps: `ARCHIVE`, `HIDDEN`, `OFFLINE`, `READONLY`, `SYSTEM` and `TEMPORARY`.
//! `FILE_ATTRIBUTE_NORMAL` is not a member, since the page says it is valid only alone and that any
//! other attribute overrides it, so it is the absence of attributes and the empty set stands for it.
//! `FILE_ATTRIBUTE_ENCRYPTED` is excluded (DI-D-64). There is no constructor from a raw number, so the
//! only masks that exist are subsets of the six.

use std::fmt;
use std::ops::{BitOr, BitOrAssign};

use windows_sys::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_ARCHIVE, FILE_ATTRIBUTE_HIDDEN, FILE_ATTRIBUTE_OFFLINE, FILE_ATTRIBUTE_READONLY,
    FILE_ATTRIBUTE_SYSTEM, FILE_ATTRIBUTE_TEMPORARY,
};

#[cfg(test)]
mod tests;

/// The attributes a file is created with: a set of `ARCHIVE`, `HIDDEN`, `OFFLINE`, `READONLY`, `SYSTEM`
/// and `TEMPORARY`, named as `CreateFileW` names `FILE_ATTRIBUTE_*` without their prefix.
///
/// ```
/// use win_durable_ioring::windows_backend::FileAttributes;
///
/// let attributes = FileAttributes::HIDDEN | FileAttributes::SYSTEM;
///
/// assert!(attributes.contains(FileAttributes::HIDDEN));
/// assert!(!attributes.contains(FileAttributes::READONLY));
/// assert!(FileAttributes::NONE.is_empty());
/// ```
///
/// They apply to a file the open creates. The `CreateFileW` page says that for a file that exists the
/// supplied attributes are generally combined with it or ignored, which is Win32's and is not
/// refined here; [`FileOptions::attributes`](super::FileOptions::attributes) says what was measured.
/// The page also says applications should not change `OFFLINE` arbitrarily.
///
/// The empty set asks for no attribute and is what an open passes when none is given; it is not
/// `FILE_ATTRIBUTE_NORMAL`, which the page defines as valid only alone.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct FileAttributes(u32);

/// Names the members once, so the constants, the names `Debug` prints and the tests that walk every
/// subset are one list and not three that can disagree.
macro_rules! attributes {
    ( $( $(#[$doc:meta])* $name:ident = $value:expr; )* ) => {
        impl FileAttributes {
            $( $(#[$doc])* pub const $name: Self = Self($value); )*
        }

        /// Every named member, in the order they are declared.
        const NAMED: &[(&str, FileAttributes)] = &[ $( (stringify!($name), FileAttributes::$name), )* ];
    };
}

attributes! {
    /// `FILE_ATTRIBUTE_ARCHIVE`: the file is marked for backup or removal.
    ARCHIVE = FILE_ATTRIBUTE_ARCHIVE;
    /// `FILE_ATTRIBUTE_HIDDEN`: the file is hidden, and not included in an ordinary directory listing.
    HIDDEN = FILE_ATTRIBUTE_HIDDEN;
    /// `FILE_ATTRIBUTE_OFFLINE`: the data is not immediately available, because it has been moved to
    /// offline storage. The `CreateFileW` page says an application should not change it arbitrarily.
    OFFLINE = FILE_ATTRIBUTE_OFFLINE;
    /// `FILE_ATTRIBUTE_READONLY`: the file can be read and not written or deleted.
    READONLY = FILE_ATTRIBUTE_READONLY;
    /// `FILE_ATTRIBUTE_SYSTEM`: the file is part of, or used exclusively by, the operating system.
    SYSTEM = FILE_ATTRIBUTE_SYSTEM;
    /// `FILE_ATTRIBUTE_TEMPORARY`: the file is for temporary storage. The page says it lets a file
    /// system keep the data in the cache and avoid writing it back while memory allows.
    TEMPORARY = FILE_ATTRIBUTE_TEMPORARY;
}

impl FileAttributes {
    /// No attribute: what an open asks for unless it is told otherwise.
    pub const NONE: Self = Self(0);

    /// The union of two sets.
    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// Whether every member of `other` is in this set.
    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// Whether the set has no member.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// The mask this set is in `dwFlagsAndAttributes`.
    pub(super) const fn bits(self) -> u32 {
        self.0
    }
}

impl BitOr for FileAttributes {
    type Output = Self;

    fn bitor(self, other: Self) -> Self {
        self.union(other)
    }
}

impl BitOrAssign for FileAttributes {
    fn bitor_assign(&mut self, other: Self) {
        *self = self.union(other);
    }
}

impl fmt::Debug for FileAttributes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("FileAttributes(")?;
        if self.is_empty() {
            f.write_str("none")?;
        } else {
            let mut first = true;
            for (name, _) in NAMED.iter().filter(|(_, member)| self.contains(*member)) {
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
