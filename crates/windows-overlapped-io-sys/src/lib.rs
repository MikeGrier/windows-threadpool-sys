// Copyright (c) 2026 Mike Grier
//! Owned overlapped I/O endpoints and pinned operations for Windows.
//!
//! This crate provides the ownership, association, completion, cancellation, and
//! rundown model for overlapped I/O on top of `windows-sys`. It is the reusable
//! foundation beneath `windows-threadpool-sys`: raw I/O completion ports and the
//! object-based thread pool share endpoint and operation storage while remaining
//! distinct completion backends.
//!
//! The crate is currently in its initial development stage. No safe API has been
//! stabilized yet, because the generic overlapped-submission safety boundary is
//! still under investigation.

#![warn(missing_docs)]

#[cfg(windows)]
mod blocking;

#[cfg(windows)]
mod config;

#[cfg(windows)]
mod endpoint;

#[cfg(all(windows, feature = "fs"))]
mod fs;

#[cfg(windows)]
mod iocp;

#[cfg(windows)]
mod operation;

#[cfg(windows)]
pub use blocking::BlockingEndpoint;

#[cfg(windows)]
pub use config::{SourceTrackingAlreadySet, set_source_tracking, source_tracking_enabled};

#[cfg(windows)]
pub use endpoint::UnassociatedEndpoint;

#[cfg(windows)]
pub use iocp::{AssociatedEndpoint, Completion, CompletionPort, Issued, OperationId, Submitted};

#[cfg(windows)]
pub use operation::{Operation, OperationState, reclaim_overlapped};
