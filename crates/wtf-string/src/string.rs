// Copyright (c) 2026 Mike Grier
//! The owned [`WtfString`] and borrowed [`WtfStr`] string types.

use std::borrow::Borrow;
use std::cmp::Ordering;
use std::fmt::{self, Debug, Display, Formatter};
use std::hash::{Hash, Hasher};
use std::ops::Deref;

use crate::encoding::{Wtf16, WtfEncoding};

/// A borrowed string slice of code units in encoding `E` (the analog of
/// [`OsStr`](std::ffi::OsStr) / [`str`]).
///
/// This is `#[repr(transparent)]` over `[E::Unit]`, so a `&WtfStr<E>` can be
/// created from a `&[E::Unit]` without copying. The units are the string's
/// *content*: there is no terminator here, since the always-terminated invariant
/// is a property of the owned [`WtfString`], not of an arbitrary borrowed slice.
#[repr(transparent)]
pub struct WtfStr<E: WtfEncoding> {
    units: [E::Unit],
}

impl<E: WtfEncoding> WtfStr<E> {
    /// Wrap a slice of code units as a `&WtfStr<E>` without copying.
    #[must_use]
    pub fn from_units(units: &[E::Unit]) -> &WtfStr<E> {
        // SAFETY: `WtfStr<E>` is `#[repr(transparent)]` over `[E::Unit]`, so the
        // two have identical layout and the slice's length metadata carries over.
        unsafe { &*(units as *const [E::Unit] as *const WtfStr<E>) }
    }

    /// The content code units (there is no terminator on a borrowed slice).
    #[must_use]
    pub fn as_units(&self) -> &[E::Unit] {
        &self.units
    }

    /// The number of content code units (not bytes, not code points).
    #[must_use]
    pub fn len(&self) -> usize {
        self.units.len()
    }

    /// Whether the string has no content units.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.units.is_empty()
    }

    /// Whether the content contains a NUL (`E::NUL`) code unit.
    ///
    /// A terminated `LPCWSTR` view of an owned string is a valid C string only
    /// when this is `false`; counted access is always valid regardless.
    #[must_use]
    pub fn has_interior_nul(&self) -> bool {
        self.units.contains(&E::NUL)
    }

    /// Decode to a `String` if the content is well-formed for this encoding.
    ///
    /// Returns `None` for content a strict `String` cannot hold (e.g. an unpaired
    /// surrogate in WTF-16); use [`to_string_lossy`](Self::to_string_lossy) to
    /// decode with replacement instead.
    #[must_use]
    pub fn to_string_checked(&self) -> Option<String> {
        E::decode(self.as_units())
    }

    /// Decode to a `String`, replacing any ill-formed sequence with `U+FFFD`.
    #[must_use]
    pub fn to_string_lossy(&self) -> String {
        E::decode_lossy(self.as_units())
    }
}

/// An owned, growable string of code units in encoding `E` (the analog of
/// [`OsString`](std::ffi::OsString) / [`String`]).
///
/// The backing buffer always carries a trailing `E::NUL` beyond the logical
/// content, so a terminated pointer for wide (`*W`) Win32 APIs is available with
/// no extra allocation, while content access (via [`Deref`] to [`WtfStr`])
/// excludes the terminator. Content may itself contain interior NULs (parity with
/// [`OsString`](std::ffi::OsString)); see [`WtfStr::has_interior_nul`].
pub struct WtfString<E: WtfEncoding> {
    // Invariant: non-empty; `units[..units.len() - 1]` is the content and the
    // final element is the always-present `E::NUL` terminator.
    units: Vec<E::Unit>,
}

impl<E: WtfEncoding> WtfString<E> {
    /// Create an empty string (a buffer holding only the terminator).
    #[must_use]
    pub fn new() -> Self {
        WtfString {
            units: vec![E::NUL],
        }
    }

    /// Create an owned string from content code units, appending the terminator.
    #[must_use]
    pub fn from_units(units: &[E::Unit]) -> Self {
        let mut buf = Vec::with_capacity(units.len() + 1);
        buf.extend_from_slice(units);
        buf.push(E::NUL);
        WtfString { units: buf }
    }

    /// The content code units, excluding the terminator.
    fn content(&self) -> &[E::Unit] {
        // The invariant guarantees at least the terminator element is present.
        &self.units[..self.units.len() - 1]
    }

    /// Build an owned string from already-encoded content units, appending the
    /// terminator without re-copying the input.
    fn from_encoded(mut units: Vec<E::Unit>) -> Self {
        units.push(E::NUL);
        WtfString { units }
    }

    /// Consume the string and decode it to a `String` if its content is
    /// well-formed for this encoding, otherwise return the original unchanged.
    ///
    /// The native-`u16` analog of
    /// [`OsString::into_string`](std::ffi::OsString::into_string).
    pub fn into_string(self) -> Result<String, Self> {
        match E::decode(self.content()) {
            Some(s) => Ok(s),
            None => Err(self),
        }
    }
}

impl<E: WtfEncoding> Deref for WtfString<E> {
    type Target = WtfStr<E>;

    fn deref(&self) -> &WtfStr<E> {
        WtfStr::from_units(self.content())
    }
}

impl<E: WtfEncoding> AsRef<WtfStr<E>> for WtfString<E> {
    fn as_ref(&self) -> &WtfStr<E> {
        self
    }
}

impl<E: WtfEncoding> AsRef<WtfStr<E>> for WtfStr<E> {
    fn as_ref(&self) -> &WtfStr<E> {
        self
    }
}

impl<E: WtfEncoding> Borrow<WtfStr<E>> for WtfString<E> {
    fn borrow(&self) -> &WtfStr<E> {
        self
    }
}

impl<E: WtfEncoding> ToOwned for WtfStr<E> {
    type Owned = WtfString<E>;

    fn to_owned(&self) -> WtfString<E> {
        WtfString::from_units(self.as_units())
    }
}

impl<E: WtfEncoding> Default for WtfString<E> {
    fn default() -> Self {
        Self::new()
    }
}

impl<E: WtfEncoding> Clone for WtfString<E> {
    fn clone(&self) -> Self {
        WtfString {
            units: self.units.clone(),
        }
    }
}

impl<E: WtfEncoding> From<&str> for WtfString<E> {
    fn from(s: &str) -> Self {
        Self::from_encoded(E::encode_str(s))
    }
}

impl<E: WtfEncoding> From<String> for WtfString<E> {
    fn from(s: String) -> Self {
        Self::from_encoded(E::encode_str(&s))
    }
}

impl<E: WtfEncoding> Display for WtfStr<E> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        Display::fmt(&self.to_string_lossy(), f)
    }
}

impl<E: WtfEncoding> Display for WtfString<E> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        Display::fmt(&**self, f)
    }
}

impl<E: WtfEncoding> Debug for WtfStr<E> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        E::debug_fmt(self.as_units(), f)
    }
}

impl<E: WtfEncoding> Debug for WtfString<E> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        Debug::fmt(&**self, f)
    }
}

// Ordering, equality, and hashing are a binary comparison of the content code
// units, so they are the same across every encoding.

impl<E: WtfEncoding> PartialEq for WtfStr<E> {
    fn eq(&self, other: &Self) -> bool {
        self.units == other.units
    }
}

impl<E: WtfEncoding> Eq for WtfStr<E> {}

impl<E: WtfEncoding> Ord for WtfStr<E> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.units.cmp(&other.units)
    }
}

impl<E: WtfEncoding> PartialOrd for WtfStr<E> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<E: WtfEncoding> Hash for WtfStr<E> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.units.hash(state);
    }
}

impl<E: WtfEncoding> PartialEq for WtfString<E> {
    fn eq(&self, other: &Self) -> bool {
        **self == **other
    }
}

impl<E: WtfEncoding> Eq for WtfString<E> {}

impl<E: WtfEncoding> Ord for WtfString<E> {
    fn cmp(&self, other: &Self) -> Ordering {
        (**self).cmp(&**other)
    }
}

impl<E: WtfEncoding> PartialOrd for WtfString<E> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<E: WtfEncoding> Hash for WtfString<E> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        (**self).hash(state);
    }
}

// Cross-type comparison with `str`: a `str` is encoded to units and compared
// exactly, so a `WtfStr` holding ill-formed units is never equal to any `str`.

impl<E: WtfEncoding> PartialEq<str> for WtfStr<E> {
    fn eq(&self, other: &str) -> bool {
        E::eq_str(self.as_units(), other)
    }
}

impl<E: WtfEncoding> PartialEq<&str> for WtfStr<E> {
    fn eq(&self, other: &&str) -> bool {
        *self == **other
    }
}

impl<E: WtfEncoding> PartialEq<str> for WtfString<E> {
    fn eq(&self, other: &str) -> bool {
        **self == *other
    }
}

impl<E: WtfEncoding> PartialEq<&str> for WtfString<E> {
    fn eq(&self, other: &&str) -> bool {
        **self == **other
    }
}

/// A [`WtfString`] whose storage is WTF-16 (`u16` code units).
pub type Wtf16String = WtfString<Wtf16>;

/// A [`WtfStr`] whose storage is WTF-16 (`u16` code units).
pub type Wtf16Str = WtfStr<Wtf16>;

#[cfg(test)]
mod tests;
