// Copyright (c) 2026 Mike Grier
//! [`ShareMode`]: what an open lets other opens of the same file do, as a typed set (DI-13.4,
//! DI-D-64).
//!
//! `CreateFileW` takes a `dwShareMode` mask of three flags, and any subset of them, including none, is
//! meaningful. Each member names one thing a later open may ask for while this handle is open; a set
//! is built by naming them and joining them. There is no constructor from a raw number, so the only
//! masks that exist are the eight subsets of the three flags the documentation lists.

use std::fmt;
use std::ops::{BitOr, BitOrAssign};

use windows_sys::Win32::Storage::FileSystem::{
    FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
};

#[cfg(test)]
mod tests;

/// What an open allows other opens of the same file to request while its handle is open: a set of
/// [`READ`](Self::READ), [`WRITE`](Self::WRITE) and [`DELETE`](Self::DELETE), named as `CreateFileW`
/// names `FILE_SHARE_READ`, `FILE_SHARE_WRITE` and `FILE_SHARE_DELETE` without their prefix.
///
/// ```
/// use win_durable_ioring::windows_backend::ShareMode;
///
/// let share = ShareMode::READ | ShareMode::WRITE;
///
/// assert!(share.contains(ShareMode::READ));
/// assert!(!share.contains(ShareMode::DELETE));
/// assert!(ShareMode::NONE.is_empty());
/// assert!(ShareMode::ALL.contains(share));
/// ```
///
/// The members are what a *later* open may ask for, and the table in the `CreateFileW` documentation
/// has two sides: a later open fails if it asks for an access this set does not share, and fails if
/// this open asked for an access that the later open's own set does not share. The empty set shares
/// nothing, so no other open may request read, write or delete access while the handle is open: an
/// exclusive open. [`FileOptions`](super::FileOptions) opens with [`ALL`](Self::ALL) unless told
/// otherwise, which is how `std::fs::OpenOptions` opens a file.
///
/// There is deliberately no `Default`: the empty set is the exclusive one and the set a file is opened
/// with is every member, so either would be a surprising thing for `default()` to answer.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct ShareMode(u32);

/// Names the flags once, so the constants, the names `Debug` prints and the tests that walk every
/// subset are one list and not three that can disagree.
macro_rules! shares {
    ( $( $(#[$doc:meta])* $name:ident = $value:expr; )* ) => {
        impl ShareMode {
            $( $(#[$doc])* pub const $name: Self = Self($value); )*
        }

        /// Every named member, in the order they are declared.
        const NAMED: &[(&str, ShareMode)] = &[ $( (stringify!($name), ShareMode::$name), )* ];
    };
}

shares! {
    /// `FILE_SHARE_READ`: later opens may request read access.
    READ = FILE_SHARE_READ;
    /// `FILE_SHARE_WRITE`: later opens may request write access.
    WRITE = FILE_SHARE_WRITE;
    /// `FILE_SHARE_DELETE`: later opens may request delete access, which allows both delete and
    /// rename.
    DELETE = FILE_SHARE_DELETE;
}

impl ShareMode {
    /// No sharing: no other open may request read, write or delete access while the handle is open.
    pub const NONE: Self = Self(0);

    /// Every member: what a file is opened with unless it is told otherwise.
    pub const ALL: Self = Self(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE);

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

    /// Whether the set shares nothing.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// The mask this set is in `dwShareMode`.
    pub(super) const fn bits(self) -> u32 {
        self.0
    }
}

impl BitOr for ShareMode {
    type Output = Self;

    fn bitor(self, other: Self) -> Self {
        self.union(other)
    }
}

impl BitOrAssign for ShareMode {
    fn bitor_assign(&mut self, other: Self) {
        *self = self.union(other);
    }
}

impl fmt::Debug for ShareMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ShareMode(")?;
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
