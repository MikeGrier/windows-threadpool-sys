// Copyright (c) 2026 Mike Grier
//! A GUID, the identifier of a provider.

use std::fmt;
use std::hash::{Hash, Hasher};

use windows_sys::core::GUID;

#[cfg(test)]
mod tests;

/// A GUID: what identifies a provider, and what a data field of that type holds.
///
/// It is built from, and read back as, the GUID's value as one big-endian number in the order its text is
/// written, so that `{12345678-9abc-def0-1234-56789abcdef0}` is `0x12345678_9abc_def0_1234_56789abcdef0`. That is
/// `windows_sys::core::GUID::from_u128`'s order and the one `windows-namespace-request-sys` takes an object id in. It
/// is not the order of the bytes of the structure, which holds its first three groups in the machine's own order;
/// a value of this type is laid out as that structure where one is written to an event.
#[derive(Clone, Copy)]
pub struct Guid(GUID);

impl Guid {
    /// The GUID whose value, read as one big-endian number, is `value`.
    #[must_use]
    pub const fn from_u128(value: u128) -> Self {
        Self(GUID::from_u128(value))
    }

    /// The GUID's value as one big-endian number, as [`from_u128`](Self::from_u128) takes it.
    #[must_use]
    pub const fn as_u128(self) -> u128 {
        let GUID {
            data1,
            data2,
            data3,
            data4,
        } = self.0;
        (data1 as u128) << 96
            | (data2 as u128) << 80
            | (data3 as u128) << 64
            | u64::from_be_bytes(data4) as u128
    }

    /// The structure the calls take.
    pub(crate) const fn raw(&self) -> &GUID {
        &self.0
    }

    /// The GUID a call gave.
    pub(crate) const fn from_raw(raw: GUID) -> Self {
        Self(raw)
    }
}

impl PartialEq for Guid {
    fn eq(&self, other: &Self) -> bool {
        self.as_u128() == other.as_u128()
    }
}

impl Eq for Guid {}

impl Hash for Guid {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_u128().hash(state);
    }
}

impl fmt::Display for Guid {
    /// As the registry and the trace tools write one: braces, lowercase hexadecimal, groups of 8, 4, 4, 4 and 12 digits.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let v = self.as_u128();
        write!(
            f,
            "{{{:08x}-{:04x}-{:04x}-{:04x}-{:012x}}}",
            (v >> 96) as u32,
            (v >> 80) as u16,
            (v >> 64) as u16,
            (v >> 48) as u16,
            v & 0xffff_ffff_ffff
        )
    }
}

impl fmt::Debug for Guid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Guid({self})")
    }
}
