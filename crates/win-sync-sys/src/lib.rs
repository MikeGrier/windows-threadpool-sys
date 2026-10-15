// Copyright (c) 2026 Mike Grier
#![doc = include_str!("../README.md")]
#![cfg(windows)]
#![deny(missing_docs)]
#![deny(unsafe_op_in_unsafe_fn)]

mod apc;
mod duplicate;
mod event;
mod signal;
#[cfg(test)]
mod test_support;
mod wait;
mod wait_many;

pub use apc::{queue_user_apc, queue_user_apc_to_current_thread};
pub use duplicate::{Access, duplicate_handle_with_access};
pub use event::{Event, ResetMode, WaitOnlyEvent};
pub use signal::{Signalable, signal_object_and_wait};
pub use wait::{Alertable, WaitOutcome, wait_for_single_object, wait_for_single_object_ex};
pub use wait_many::{MAXIMUM_OBJECTS, WaitAnyOutcome, wait_for_all, wait_for_any};
