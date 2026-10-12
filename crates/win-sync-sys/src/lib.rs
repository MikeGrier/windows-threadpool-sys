// Copyright (c) 2026 Mike Grier
#![doc = include_str!("../README.md")]
#![cfg(windows)]
#![deny(missing_docs)]
#![deny(unsafe_op_in_unsafe_fn)]

mod duplicate;
mod event;
mod wait;

pub use duplicate::{Access, duplicate_handle_with_access};
pub use event::{Event, ResetMode, WaitOnlyEvent};
pub use wait::{WaitOutcome, wait_for_single_object};
