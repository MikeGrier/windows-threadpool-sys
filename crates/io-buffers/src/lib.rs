// Copyright (c) 2026 Mike Grier
#![doc = include_str!("../README.md")]
#![deny(missing_docs)]
#![deny(unsafe_op_in_unsafe_fn)]
// Every `unsafe` here states why it is sound.
#![deny(clippy::undocumented_unsafe_blocks)]

mod buf;
mod span;

pub use buf::{IoBuf, IoBufMut};
pub use span::RegisteredSpan;
