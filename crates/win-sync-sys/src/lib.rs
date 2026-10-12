// Copyright (c) 2026 Mike Grier
#![doc = include_str!("../README.md")]
#![cfg(windows)]
#![deny(missing_docs)]
#![deny(unsafe_op_in_unsafe_fn)]

mod event;
mod wait;

pub use event::{Event, ResetMode};
pub use wait::{WaitOutcome, wait_for_single_object};
