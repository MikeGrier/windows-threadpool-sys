// Copyright (c) 2026 Mike Grier
//! Windows `OsStr` / `OsString` interop for the WTF-16 string types.
//!
//! On Windows an `OsStr` is stored as WTF-8, so a bridge to WTF-16 cannot borrow
//! -- it converts once at the boundary via `encode_wide` / `from_wide`. Both
//! directions are lossless, including for unpaired surrogates, because `OsStr` and
//! `WtfStr<Wtf16>` are both WTF supersets (D-5/D-8). A borrowing `AsRef<OsStr>` is
//! deliberately *not* provided: the two types have different backing widths, so no
//! zero-copy `&OsStr` view of `u16` storage exists (see DESIGN-NOTES D-14).

use std::ffi::{OsStr, OsString};
use std::os::windows::ffi::{OsStrExt, OsStringExt};

use super::{Wtf16Str, Wtf16String};

impl Wtf16String {
    /// Encode an [`OsStr`] into owned WTF-16, converting once at the boundary.
    ///
    /// Lossless: unpaired surrogates survive, so
    /// `from_os_str(x).to_os_string() == x`. This is the owning analog of
    /// collecting [`OsStrExt::encode_wide`].
    #[must_use]
    pub fn from_os_str(s: &OsStr) -> Self {
        Self::from_encoded(s.encode_wide().collect())
    }

    /// Build from already-wide code units: the [`OsStringExt::from_wide`] analog.
    ///
    /// Identical to [`from_units`](Self::from_units), named for drop-in
    /// familiarity when replacing an `OsString::from_wide` call site.
    #[must_use]
    pub fn from_wide(units: &[u16]) -> Self {
        Self::from_units(units)
    }
}

impl Wtf16Str {
    /// Decode the content into an owned [`OsString`], losslessly.
    ///
    /// The [`OsStringExt::from_wide`] bridge: unpaired surrogates are preserved.
    #[must_use]
    pub fn to_os_string(&self) -> OsString {
        OsString::from_wide(self.as_units())
    }

    /// Iterate the content as wide code units, zero-copy: the
    /// [`OsStrExt::encode_wide`] analog over our own slice.
    pub fn encode_wide(&self) -> std::iter::Copied<std::slice::Iter<'_, u16>> {
        self.as_units().iter().copied()
    }
}

impl From<&OsStr> for Wtf16String {
    fn from(s: &OsStr) -> Self {
        Self::from_os_str(s)
    }
}

impl From<&OsString> for Wtf16String {
    fn from(s: &OsString) -> Self {
        Self::from_os_str(s)
    }
}

impl From<&Wtf16Str> for OsString {
    fn from(s: &Wtf16Str) -> Self {
        s.to_os_string()
    }
}

impl From<&Wtf16String> for OsString {
    fn from(s: &Wtf16String) -> Self {
        s.to_os_string()
    }
}

impl From<Wtf16String> for OsString {
    fn from(s: Wtf16String) -> Self {
        s.to_os_string()
    }
}

#[cfg(test)]
mod tests;
