// Copyright (c) 2026 Mike Grier
//! `windows`-crate `Param<PCWSTR>` interop, behind the `windows-core` feature.
//!
//! The high-level `windows` crate spells a wide string parameter as
//! `impl Param<PCWSTR>` rather than a bare `*const u16`, so a call site cannot
//! hand it our type without a conversion unless we satisfy that bound. This
//! module supplies exactly that (D-10): the terminated pointer that
//! [`Wtf16String`] already keeps for free (D-7) is wrapped in a `PCWSTR` and
//! handed over, so the zero-conversion, zero-allocation path reaches `windows`
//! call sites as well as raw `windows-sys` ones.
//!
//! See DESIGN-NOTES `D-17` for the two constraints this seam carries: it is
//! written against one `windows-core` version, and it binds to that crate's
//! `#[doc(hidden)]` `Param` machinery because implementing the trait is the only
//! way to satisfy the bound.

use windows_core::{PCWSTR, Param, ParamValue};

use super::Wtf16String;

/// Pass a `&Wtf16String` directly to a `windows` API taking `impl Param<PCWSTR>`.
///
/// The pointer handed over is [`as_terminated_ptr`](Wtf16String::as_terminated_ptr):
/// no conversion, no allocation, and no copy. It stays valid for the call because
/// the borrow keeps the owning `Wtf16String` alive and unmodified.
///
/// This carries exactly the C-string caveat of the pointer it wraps (D-7): the
/// callee stops at the first NUL, so a value with an interior NUL is seen
/// truncated. Check [`has_interior_nul`](super::WtfStr::has_interior_nul) first
/// when the content may contain one. `&HSTRING`'s own `Param<PCWSTR>` impl has
/// the same property, so this is parity with the ecosystem, not a new hazard.
///
/// There is deliberately **no** impl for `&Wtf16Str`: a borrowed slice carries no
/// terminator (D-7), so it has no valid `PCWSTR` to give. Borrowed content
/// reaches Win32 through the counted pair
/// [`as_ptr`](super::Wtf16Str::as_ptr) + [`len`](super::WtfStr::len) instead.
impl Param<PCWSTR> for &Wtf16String {
    unsafe fn param(self) -> ParamValue<PCWSTR> {
        // `Owned` names the *`PCWSTR` value*, not the string data: `PCWSTR` is a
        // `Copy` pointer wrapper that owns nothing and is never freed by the
        // callee. This mirrors `&HSTRING`'s impl, which likewise wraps a borrowed
        // interior pointer. The borrow in `self` is what keeps the data alive.
        ParamValue::Owned(PCWSTR(self.as_terminated_ptr()))
    }
}

#[cfg(test)]
mod tests;
