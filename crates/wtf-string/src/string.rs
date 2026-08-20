// Copyright (c) 2026 Mike Grier
//! The owned [`WtfString`] and borrowed [`WtfStr`] string types.

use std::borrow::Borrow;
use std::cmp::Ordering;
use std::fmt::{self, Debug, Display, Formatter};
use std::hash::{Hash, Hasher};
use std::ops::Deref;

use crate::encoding::{Wtf8, Wtf16, WtfEncoding};

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

    /// Build an owned string from already-encoded content units by appending the
    /// terminator. The encoded vector becomes the backing buffer; the final `push`
    /// may reallocate if it had no spare capacity.
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

/// A [`WtfString`] whose storage is WTF-8 (`u8` code units).
pub type Wtf8String = WtfString<Wtf8>;

/// A [`WtfStr`] whose storage is WTF-8 (`u8` code units).
pub type Wtf8Str = WtfStr<Wtf8>;

// FFI surface specific to WTF-16 storage: `*const u16` is the `windows-sys`
// `PCWSTR`/`LPCWSTR` shape (D-10). These live on the concrete instantiations
// because a raw `u16` pointer only makes sense for the `Wtf16` width (D-2).

impl Wtf16Str {
    /// A pointer to the content code units, for counted FFI paired with
    /// [`len`](WtfStr::len).
    ///
    /// The pointer is **not** guaranteed to be NUL-terminated (a borrowed slice
    /// carries no terminator); use it only with the matching unit count. It is
    /// valid while `self` is borrowed and unmodified. For a terminated
    /// `LPCWSTR`, start from an owned [`Wtf16String`] and use
    /// [`Wtf16String::as_terminated_ptr`].
    #[must_use]
    pub fn as_ptr(&self) -> *const u16 {
        self.as_units().as_ptr()
    }
}

impl Wtf16String {
    /// A NUL-terminated `*const u16` (`LPCWSTR`/`PCWSTR`) over the whole buffer.
    ///
    /// The always-present terminator makes this allocation-free (D-7). It is a
    /// valid C string only when [`has_interior_nul`](WtfStr::has_interior_nul) is
    /// `false`; otherwise a reader stops at the first interior NUL. The pointer is
    /// valid while `self` is borrowed and unmodified.
    #[must_use]
    pub fn as_terminated_ptr(&self) -> *const u16 {
        // The buffer is `[content.., NUL]`, so its first element is the start of
        // a terminated string.
        self.units.as_ptr()
    }

    /// An empty string with room for `units` content code units to be filled in
    /// place via [`as_mut_ptr`](Self::as_mut_ptr) plus
    /// [`set_len_from_ffi`](Self::set_len_from_ffi).
    ///
    /// The reserved capacity also covers the always-present terminator, so a
    /// later [`set_len_from_ffi`](Self::set_len_from_ffi) of up to `units` content
    /// units re-establishes the invariant without reallocating (D-9).
    #[must_use]
    pub fn with_capacity(units: usize) -> Self {
        // Reserve content + terminator up front, then seed the empty-string
        // invariant `[NUL]`; the spare capacity is where a foreign buffer-fill
        // writes. `checked_add` guards the `usize::MAX` edge that would otherwise
        // wrap to a tiny allocation in release builds.
        let capacity = units.checked_add(1).expect("capacity overflow");
        let mut buf = Vec::with_capacity(capacity);
        buf.push(Wtf16::NUL);
        WtfString { units: buf }
    }

    /// A mutable pointer to the start of the buffer, for a foreign buffer-fill.
    ///
    /// [`with_capacity`](Self::with_capacity)`(n)` reserves `n + 1` units: room
    /// for `n` content units plus the terminator slot. A foreign API may fill up
    /// to `n` content units, and one more if it writes its own terminator into
    /// the reserved slot (`n + 1` units total). Either way, pass only the
    /// **content** length to [`set_len_from_ffi`](Self::set_len_from_ffi), which
    /// publishes that length and re-establishes the terminator.
    ///
    /// Writing through this pointer overwrites the buffer -- including element 0,
    /// which is the sole terminator of a fresh `with_capacity` -- so it **breaks
    /// the always-terminated invariant** until
    /// [`set_len_from_ffi`](Self::set_len_from_ffi) restores it. Between the write
    /// and that call the value must **not** be observed through any other method
    /// ([`as_terminated_ptr`](Self::as_terminated_ptr), [`Deref`] content access,
    /// `Clone`, `Debug`, `PartialEq`, ...): they could read a non-terminated or
    /// partially written buffer. This holds on **failure paths too** -- if the
    /// foreign call fails, restore the invariant with `set_len_from_ffi(0)` (the
    /// empty string) or drop the value before any other use. The pointer is valid
    /// while `self` is borrowed and not reallocated.
    #[must_use]
    pub fn as_mut_ptr(&mut self) -> *mut u16 {
        self.units.as_mut_ptr()
    }

    /// Publish `content_units` content code units written into the buffer from
    /// [`as_mut_ptr`](Self::as_mut_ptr), then append the terminator.
    ///
    /// `content_units` counts **content only** and never includes a terminator.
    /// The written units are taken verbatim -- they may themselves end in `NUL`,
    /// since interior NULs are permitted (see
    /// [`has_interior_nul`](WtfStr::has_interior_nul)) -- and exactly one
    /// terminator is appended. A foreign API that reports a count *including* the
    /// terminator it wrote must subtract one and pass the content length; this
    /// method never inspects the buffer to guess the convention, so a genuine
    /// trailing content `NUL` is never mistaken for the terminator.
    ///
    /// # Safety
    ///
    /// The caller must guarantee that:
    /// - the first `content_units` code units at [`as_mut_ptr`](Self::as_mut_ptr)
    ///   are initialized `u16` values, and
    /// - `content_units` does not exceed the count requested via
    ///   [`with_capacity`](Self::with_capacity), so the appended terminator fits
    ///   without reallocating a buffer whose pointer the caller may still hold.
    pub unsafe fn set_len_from_ffi(&mut self, content_units: usize) {
        // `content_units < capacity` guards both `set_len` soundness and the room
        // to append the terminator without reallocating (which would strand a
        // pointer handed out via `as_mut_ptr`).
        debug_assert!(
            content_units < self.units.capacity(),
            "set_len_from_ffi content length leaves no room for the terminator"
        );
        // SAFETY: the caller guarantees `content_units` initialized code units,
        // within capacity, so this length names only initialized storage.
        unsafe { self.units.set_len(content_units) };
        self.units.push(Wtf16::NUL);
    }

    /// Copy `len` content code units from a foreign `*const u16` into a new owned
    /// string, appending the terminator.
    ///
    /// For callee-allocated Win32 output: the bytes are **copied**, so the caller
    /// keeps ownership of (and remains responsible for freeing) the source buffer.
    /// The copy is lossless — arbitrary WTF-16, including unpaired surrogates, is
    /// preserved (D-4/D-9).
    ///
    /// # Safety
    ///
    /// This copies the range through `std::slice::from_raw_parts` and shares its
    /// preconditions. When `len > 0` the caller must guarantee that:
    /// - `ptr` is non-null and properly aligned for `u16`;
    /// - `ptr` is valid for reads of `len` consecutive, initialized `u16` values,
    ///   all contained within a **single allocated object**;
    /// - the total size `len * size_of::<u16>()` is no larger than `isize::MAX`,
    ///   and adding it to `ptr` does not wrap the address space; and
    /// - that region stays unmutated for the duration of the call.
    ///
    /// When `len == 0` the pointer is not dereferenced, so it may be null or
    /// dangling. No reference to `ptr` is retained past the call. `len` is a
    /// **count of code units**, not bytes, and excludes any terminator the callee
    /// may have written (pass the content length).
    #[must_use]
    pub unsafe fn from_wide_ptr(ptr: *const u16, len: usize) -> Self {
        if len == 0 {
            // `slice::from_raw_parts` forbids a null (or dangling) pointer even at
            // zero length, so an empty result must not touch `ptr` at all.
            return Self::new();
        }
        // SAFETY: `len > 0`, and the caller guarantees `ptr` is non-null, valid,
        // and aligned for `len` reads; the slice is used only to copy and is not
        // retained.
        let content = unsafe { std::slice::from_raw_parts(ptr, len) };
        Self::from_units(content)
    }
}

// Windows `OsStr` / `OsString` interop is the only platform-gated surface (D-5).
#[cfg(windows)]
mod os_str;

#[cfg(test)]
mod tests;
