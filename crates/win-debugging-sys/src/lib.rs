// Copyright (c) 2026 Mike Grier
#![doc = include_str!("../README.md")]
#![cfg(windows)]
#![deny(missing_docs)]
#![deny(unsafe_op_in_unsafe_fn)]

#[cfg(feature = "test-util")]
mod capture;
mod output;

#[cfg(feature = "test-util")]
pub use capture::{Captured, DebugString, run_debugged};
pub use output::{OutputDebugStringError, output_debug_str, output_debug_string};
