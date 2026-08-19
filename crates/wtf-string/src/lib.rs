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
//! The core is generic over a [code-unit encoding](WtfEncoding): `WtfString<E>`
//! owns the units and `WtfStr<E>` is the borrowed slice. v1 ships only the
//! `Wtf16` encoding, exposed as the aliases `Wtf16String` / `Wtf16Str`; the
//! `Wtf8` arm — a `u8`/WTF-8 storage variant whose encode/decode, comparison, and
//! formatting semantics this crate defines (std `OsString` is the intended
//! backing implementation, since its WTF-8 storage matches) — is a designed-in
//! seam that is not yet implemented.
//!
//! The storage and `str`/`String` conversions are portable; the `OsStr` /
//! `OsString` interop is Windows-only.
//!
//! See the crate's design records for the full set of decisions.

#![warn(missing_docs)]

mod encoding;
mod string;

pub use encoding::{Wtf16, WtfEncoding};
pub use string::{Wtf16Str, Wtf16String, WtfStr, WtfString};
