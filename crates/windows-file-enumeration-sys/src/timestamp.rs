// Copyright (c) 2026 Mike Grier
//! Native Windows timestamps, kept native.
//!
//! A directory record reports each of its four times as a signed count of
//! 100-nanosecond intervals since 1601-01-01 UTC. [`WindowsFileTimestamp`] is
//! that count and nothing else: no epoch shift, no saturation, no timezone, and
//! no reinterpretation of a zero or negative value.
//!
//! Converting eagerly to a Unix epoch would be lossy in three directions at
//! once -- range, precision, and sentinel meaning -- and the loss would be
//! unrecoverable by the time a caller saw it. A caller that wants civil time
//! converts at the point it knows which of those trade-offs it can accept.

use std::fmt;

use windows_sys::Win32::Foundation::FILETIME;

/// A Windows file time: signed 100-nanosecond ticks since 1601-01-01 UTC.
///
/// Ordering is ordering of the raw tick count, which is what makes a timestamp
/// comparison in a query mean the same thing as a comparison of the underlying
/// record fields. Filesystem sentinels participate as their raw values: a `0`
/// change time on a filesystem that does not track one compares as less than
/// every real time, and is not silently promoted to "unknown".
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WindowsFileTimestamp(i64);

impl WindowsFileTimestamp {
    /// The zero tick count, which is 1601-01-01 UTC and the value filesystems
    /// commonly report for a time they do not track.
    pub const ZERO: Self = Self(0);

    /// Wrap a raw tick count.
    #[must_use]
    pub const fn from_ticks(ticks: i64) -> Self {
        Self(ticks)
    }

    /// The raw tick count.
    #[must_use]
    pub const fn ticks(self) -> i64 {
        self.0
    }

    /// Convert from the Microsoft two-word [`FILETIME`] representation.
    ///
    /// Directory records carry these times as a single `i64` already, so this
    /// exists for interoperation with the many Win32 APIs that hand out a
    /// `FILETIME` instead -- not because the crate stores one.
    #[must_use]
    pub const fn from_filetime(time: FILETIME) -> Self {
        // A mutation run reports `|` here as replaceable by `^`, and it is an
        // equivalent mutant rather than a gap: the shift puts the high word in
        // bits 32..64 and the low word occupies bits 0..32, so the two operands
        // share no set bit and `|`, `^`, and `+` all agree on every input. No
        // test can distinguish them, and one written to try would be asserting
        // a property the code does not have.
        let ticks = ((time.dwHighDateTime as u64) << 32) | (time.dwLowDateTime as u64);
        Self(ticks as i64)
    }

    /// Convert to the Microsoft two-word [`FILETIME`] representation.
    #[must_use]
    pub const fn to_filetime(self) -> FILETIME {
        let ticks = self.0 as u64;
        FILETIME {
            dwLowDateTime: ticks as u32,
            dwHighDateTime: (ticks >> 32) as u32,
        }
    }
}

impl fmt::Display for WindowsFileTimestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ticks", self.0)
    }
}

impl From<i64> for WindowsFileTimestamp {
    fn from(ticks: i64) -> Self {
        Self(ticks)
    }
}

impl From<WindowsFileTimestamp> for i64 {
    fn from(timestamp: WindowsFileTimestamp) -> Self {
        timestamp.0
    }
}

#[cfg(test)]
mod tests;
