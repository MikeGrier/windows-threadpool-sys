// Copyright (c) 2026 Mike Grier
#![doc = include_str!("../README.md")]
#![cfg(windows)]
#![deny(missing_docs)]
#![deny(unsafe_op_in_unsafe_fn)]
// Every `unsafe` here is a Win32 call or a handle query, and each states why it is sound (TW-1.1).
#![deny(clippy::undocumented_unsafe_blocks)]

mod chunked;
mod create_error;
mod diagnostics;
mod doorbell;
mod held_file;
mod open_error;
mod poll_error;
mod pool;
mod probe_failure;
mod read_sq;
mod sq;
#[cfg(test)]
mod test_support;
mod watcher;

pub use create_error::CreateError;
pub use diagnostics::{Diagnostic, DiagnosticKind, DiagnosticSink, NoDiagnostics};
pub use held_file::HeldFile;
pub use open_error::OpenError;
pub use poll_error::PollError;
pub use sq::{Attention, SqCapacityError, SqDrain, SqEntry, Terminal};
pub use watcher::{Registration, Watcher};
