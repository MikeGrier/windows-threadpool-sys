// Copyright (c) 2026 Mike Grier
#![doc = include_str!("../README.md")]
#![cfg(windows)]
#![deny(missing_docs)]
#![deny(unsafe_op_in_unsafe_fn)]

mod duplicate;
mod handle_information;

pub use duplicate::{Access, duplicate_handle_with_access};
pub use handle_information::{HandleFlags, get_handle_information, set_handle_information};
