// Copyright (c) 2026 Mike Grier
//! `OsString`-shaped strings with native, conversion-free `u16` storage.
//!
//! Rust's [`OsString`](std::ffi::OsString) stores **WTF-8** on Windows, so every
//! call into a wide (`*W`) Win32 API pays a WTF-8 -> UTF-16 re-encode and an
//! allocation on the way out, and the reverse on the way in. For code that lives
//! in `u16` and calls Windows APIs repeatedly, that conversion is pure overhead.
//!
//! This crate provides an `OsString`-shaped type whose canonical storage is
//! `[u16]` (**WTF-16**: arbitrary, ill-formed-surrogate-tolerant UTF-16, exactly
//! what NTFS and the Win32 APIs traffic in), so the conversion happens **once at
//! the boundary from `str`/`OsStr`** and never again. The analog of
//! `OsStrExt::encode_wide` becomes a zero-copy borrow of our own slice.
//!
//! # Shape
//!
//! The core is generic over a [code-unit encoding][WtfEncoding]: `WtfString<E>`
//! owns the units and `WtfStr<E>` is the borrowed slice. Two encodings ship: the
//! `Wtf16` width (aliases `Wtf16String` / `Wtf16Str`) and the `Wtf8` width
//! (aliases `Wtf8String` / `Wtf8Str`), a `u8`/WTF-8 storage variant whose
//! encode/decode, comparison, and formatting semantics this crate defines, backed
//! by a crate-owned `Vec<u8>` (its WTF-8 storage matches `OsString`'s, but the arm
//! is not built on `OsString`).
//!
//! The storage and `str`/`String` conversions are portable; the `OsStr` /
//! `OsString` interop is Windows-only.
//!
//! ```
//! use wtf_string::Wtf16String;
//!
//! // Encode once, at the boundary.
//! let s = Wtf16String::from("C:\\Windows");
//!
//! // From here on the units are the storage: no re-encode, no allocation.
//! assert_eq!(s.len(), 10);
//! assert_eq!(s.as_units()[0], u16::from(b'C'));
//!
//! // And back again when a `String` is genuinely wanted.
//! assert_eq!(s.to_string_lossy(), "C:\\Windows");
//! ```
//!
//! # Conversion costs
//!
//! The point of the crate is that the middle rows are free. Everything that
//! costs anything is a boundary crossing you asked for:
//!
//! | Operation | Cost |
//! |---|---|
//! | `Wtf16String::from(&str)` / `from(String)` | encode + allocate, once |
//! | `Wtf16String::from_os_str` (Windows) | encode + allocate, once |
//! | [`as_ptr`](Wtf16Str::as_ptr) + [`len`](WtfStr::len), for a counted `*W` call | **free** |
//! | [`as_terminated_ptr`](Wtf16String::as_terminated_ptr), for an `LPCWSTR` call | **free** |
//! | `encode_wide` (Windows), the `OsStrExt` analog | **free** (borrows our slice) |
//! | [`as_units`](WtfStr::as_units) / [`len`](WtfStr::len) / comparison / hashing | **free** |
//! | [`to_string_checked`](WtfStr::to_string_checked) / [`to_string_lossy`](WtfStr::to_string_lossy) | decode + allocate |
//! | `to_os_string` (Windows) | re-encode + allocate |
//!
//! Compare `OsString`, where the *first* two rows are free and the wide-call
//! rows are the ones that re-encode. The two types are mirror images; which one
//! is right depends on whether your code spends its time in `str` or in Win32.
//!
//! # The FFI surface
//!
//! Both directions of a Win32 call are covered without leaving `u16`.
//!
//! **Input** -- pick the convention the signature wants:
//!
//! - counted (`ptr` + `cch`): [`as_ptr`](Wtf16Str::as_ptr) with
//!   [`len`](WtfStr::len);
//! - NUL-terminated (`LPCWSTR` / `PCWSTR`):
//!   [`as_terminated_ptr`](Wtf16String::as_terminated_ptr). The terminator is
//!   always present in the owned buffer, so this never allocates.
//!
//! **Output** -- pick the shape the API uses:
//!
//! - caller-allocated buffer-fill: [`with_capacity`](Wtf16String::with_capacity),
//!   [`as_mut_ptr`](Wtf16String::as_mut_ptr), then
//!   [`set_len_from_ffi`](Wtf16String::set_len_from_ffi) to publish the content
//!   length the API reported;
//! - callee-allocated buffer: [`from_wide_ptr`](Wtf16String::from_wide_ptr),
//!   which copies, leaving the caller to free the original.
//!
//! See `examples/win32_round_trip.rs` for both halves against a real Win32 call.
//!
//! # Interior NULs
//!
//! Content may contain NUL, matching `OsString` and the underlying WTF model.
//! The trailing terminator is *storage*, not content: it is excluded from
//! [`len`](WtfStr::len), [`as_units`](WtfStr::as_units), comparison and hashing.
//!
//! One consequence is worth knowing before using the terminated pointer: a C
//! string ends at the first NUL, so a callee reading
//! [`as_terminated_ptr`](Wtf16String::as_terminated_ptr) sees a value with an
//! interior NUL *truncated*. Counted access is always exact.
//! [`has_interior_nul`](WtfStr::has_interior_nul) reports the condition when it
//! matters.
//!
//! A checked, no-interior-NUL companion type -- one that makes "this really is a
//! valid C string" a type-level guarantee rather than a precondition to check --
//! is a **reserved** seam for a future release, deliberately outside the v1
//! surface. Until then, pair `has_interior_nul` with the terminated pointer, or
//! use the counted pair, which is never ambiguous.
//!
//! # Features
//!
//! - **`std`** (on by default) -- adds the Windows `OsStr` / `OsString` interop.
//!   `OsStr` lives in `std` and has no `alloc`-only equivalent, so it is the one
//!   part of the crate that needs it. With this feature off the crate is
//!   `no_std` + `alloc`: storage, `str` / `String` conversions, the mutation
//!   surface and the whole FFI pointer surface all still work.
//! - **`windows-core`** (off by default) -- implements the high-level `windows`
//!   crate's `Param<PCWSTR>` for `&Wtf16String`, so a `windows` API taking
//!   `impl Param<PCWSTR>` accepts our type directly, handing over the
//!   already-terminated pointer with no conversion, allocation or copy. Raw
//!   `windows-sys` signatures need no feature: they take `*const u16`, which
//!   [`Wtf16String::as_terminated_ptr`] already provides. The impl is written
//!   against one `windows-core` version, so a caller must resolve to that same
//!   semver-compatible version for it to apply.
//!
//! The crate is `#![no_std]` at its root and pulls in `alloc`. Unless the
//! `windows-core` feature is on, it has **zero dependencies**.
//!
//! See the crate's design records for the full set of decisions.

#![no_std]
#![warn(missing_docs)]

extern crate alloc;

// `std` is linked when the `std` feature is on (the `OsStr` interop needs it),
// under `cfg(test)` (the harness and tests use `HashMap`, `DefaultHasher`, ...),
// and under `cfg(doc)` so intra-doc links to `std` items resolve even in an
// `alloc`-only documentation build. The portable core itself never uses it.
#[cfg(any(feature = "std", test, doc))]
extern crate std;

mod encoding;
mod string;

pub use encoding::{Wtf8, Wtf16, WtfEncoding};
pub use string::{Wtf8Str, Wtf8String, Wtf16Str, Wtf16String, WtfStr, WtfString};
