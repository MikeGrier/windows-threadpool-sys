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
