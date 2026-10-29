// Copyright (c) 2026 Mike Grier
#![doc = include_str!("../README.md")]
#![no_std]
#![deny(missing_docs)]
#![deny(unsafe_op_in_unsafe_fn)]
// Every `unsafe` here states why it is sound.
#![deny(clippy::undocumented_unsafe_blocks)]

// The shared error counts its references in an `AtomicU64` (RCE-D-4), so this crate requires
// 64-bit atomics. A target without them, such as `thumbv7em-none-eabi`, is not served: the
// engineer chose that on 2026-10-29. This turns what would be a bare "no `AtomicU64` in
// `core::sync::atomic`" into a message that says why, and what to build for instead.
#[cfg(not(target_has_atomic = "64"))]
compile_error!(
    "rc-errors requires 64-bit atomics (`target_has_atomic = \"64\"`): the shared error counts \
     its references in an `AtomicU64`. This target does not have them and is not supported; \
     build for a target that does, such as `x86_64-unknown-none`."
);

// `Global` allocates through `alloc::alloc`; nothing else here allocates except through an
// allocator it is given (RCE-D-2).
extern crate alloc;
#[cfg(test)]
extern crate std;

mod allocator;
mod report;
mod shared;
mod static_error;

pub use allocator::{AllocError, Allocator, Global, Refusing};
pub use report::ErrorReport;
pub use shared::SharedError;
pub use static_error::StaticError;
