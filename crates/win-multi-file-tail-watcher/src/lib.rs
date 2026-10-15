// Copyright (c) 2026 Mike Grier
#![doc = include_str!("../README.md")]
#![cfg(windows)]
#![deny(missing_docs)]
#![deny(unsafe_op_in_unsafe_fn)]
// Every `unsafe` here is a Win32 call or a handle query, and each states why it is sound (TW-1.1).
#![deny(clippy::undocumented_unsafe_blocks)]

mod held_file;
mod open_error;
#[cfg(test)]
mod test_support;
mod watcher;

pub use held_file::HeldFile;
pub use open_error::OpenError;
pub use watcher::{Registration, Watcher};
