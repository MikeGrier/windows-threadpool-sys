// Copyright (c) 2026 Mike Grier
#![doc = include_str!("../README.md")]
#![no_std]
#![deny(missing_docs)]
#![deny(unsafe_op_in_unsafe_fn)]
// Every `unsafe` here states why it is sound.
#![deny(clippy::undocumented_unsafe_blocks)]

// `Global` allocates through `alloc::alloc`; nothing else here allocates except through an
// allocator it is given (RCE-D-2).
extern crate alloc;
#[cfg(test)]
extern crate std;

mod allocator;
mod shared;

pub use allocator::{AllocError, Allocator, Global};
pub use shared::SharedError;
