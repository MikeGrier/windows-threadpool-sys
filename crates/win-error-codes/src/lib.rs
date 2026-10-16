// Copyright (c) Mike Grier.

//! The numbers Windows reports failure in, as types.
//!
//! Windows reports a failure as a number, and which number space it is in is not in the number.
//! A `DWORD` from `GetLastError` is one; a `LONG` from a registry call is the same space in a
//! signed carrier; an `HRESULT` is another, signed and carrying severity and facility bits.
//! Crates here each kept a bare `u32` or wrote a `Win32Error` of their own, and the two that did
//! disagreed about what to do with a code of 2^31 or more -- which exist: the virtdisk errors are
//! `WIN32_ERROR`s such as `ERROR_VIRTDISK_NOT_VIRTUAL_DISK`, 3225026581.
//!
//! This crate gives each number space its own type, so a code is never a bare integer whose space
//! the reader has to guess, and so the conversion to and from [`std::io::Error`] is written once
//! and is exact for every code.
//!
//! The decisions behind it are in the crate's `DESIGN-NOTES.md`. The types carry what Windows
//! said and nothing else: no code is mapped onto a friendlier taxonomy, and no named constants
//! are defined here, because `windows-sys` has them.
//!
//! The numbers and their conversions build on every target. The bridge to [`std::io::Error`] is
//! `cfg(windows)`, because off Windows a raw OS error is an `errno` and reading one as a Win32
//! code would be a wrong answer and not a missing feature.

mod hresult;
mod lstatus;
mod win32_error;

#[cfg(test)]
mod test_codes;

pub use hresult::HResult;
pub use lstatus::LStatus;
pub use win32_error::Win32Error;

// The README carries runnable examples, so they are compiled and a change to the API that breaks
// one breaks the build.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
struct ReadmeDoctests;
