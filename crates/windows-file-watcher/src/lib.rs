// Copyright (c) 2026 Mike Grier
//! Memory-safe Windows path-change watching with full `ReadDirectoryChangesW`
//! fidelity and a `FindFirstChangeNotification` coarse fallback.
//!
//! **In development: this crate currently ships only the notification decoder**
//! ([`decode_batch`] and the [`Change`] / [`ChangeKind`] / [`DecodedBatch`] /
//! [`DesyncCause`] / [`RelativeName`] types it yields). The queue-mediated
//! `Monitor` / `Session` / `Watch` model and the coarse fallback described below
//! are the planned surface, built out across later milestones; they are not yet
//! available.
//!
//! This crate is Windows-only: every item is gated behind `cfg(windows)`, so it
//! resolves to an empty crate on other targets. Platform-independent watching is
//! meant to be built at a higher layer -- this crate is about excellent Windows
//! behaviour (path-name and notification-limitation fidelity) with memory safety.
//!
//! The planned public surface is a queue-mediated `Monitor` / `Session` / `Watch`
//! model, built out across milestones; its design decisions, rationale, and
//! schedule are recorded in the crate's source repository.

#![warn(missing_docs)]

#[cfg(windows)]
mod directory;

#[cfg(windows)]
mod notify;

#[cfg(windows)]
mod queue;

#[cfg(windows)]
mod watcher;

#[cfg(windows)]
pub use notify::{Change, ChangeKind, DecodedBatch, DesyncCause, RelativeName, decode_batch};
