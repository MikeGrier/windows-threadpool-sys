// Copyright (c) 2026 Mike Grier
//! Memory-safe Windows path-change watching with full `ReadDirectoryChangesW`
//! fidelity and a `FindFirstChangeNotification` coarse fallback.
//!
//! This crate is Windows-only: every item is gated behind `cfg(windows)`, so it
//! resolves to an empty crate on other targets. Platform-independent watching is
//! meant to be built at a higher layer -- this crate is about excellent Windows
//! behaviour (path-name and notification-limitation fidelity) with memory safety.
//!
//! The public surface is a queue-mediated `Monitor` / `Session` / `Watch` model
//! and is built out across milestones; see [CHECKLIST.md](../CHECKLIST.md) and the
//! design sessions in [design-sessions/](../design-sessions/) for the shape and
//! the decisions behind it.

#![warn(missing_docs)]

#[cfg(windows)]
mod notify;

#[cfg(windows)]
pub use notify::{Change, ChangeKind, DecodedBatch, DesyncCause, RelativeName, decode_batch};
