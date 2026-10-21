// Copyright (c) 2026 Mike Grier
#![doc = include_str!("../README.md")]
#![cfg(windows)]
#![deny(missing_docs)]
#![deny(unsafe_op_in_unsafe_fn)]

mod output;

pub use output::{OutputDebugStringError, output_debug_str, output_debug_string};
