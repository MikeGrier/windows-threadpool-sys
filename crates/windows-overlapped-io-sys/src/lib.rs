// Copyright (c) 2026 Mike Grier
//! Owned overlapped I/O endpoints and pinned operations for Windows.
//!
//! This crate provides the ownership, association, completion, cancellation, and
//! rundown model for overlapped I/O on top of `windows-sys`. It is the reusable
//! foundation beneath `windows-threadpool-sys`: raw I/O completion ports and the
//! object-based thread pool share endpoint and operation storage while remaining
//! distinct completion backends.
//!
//! # Safe API surface
//!
//! Endpoints are created safely with [`UnassociatedEndpoint::open`], and each
//! operation family has safe adapters behind an opt-in feature, so callers issue
//! real overlapped I/O without writing `unsafe`:
//!
//! - `fs`: file read/write and scatter/gather, on the blocking and IOCP backends.
//! - `socket`: socket send/receive, on the blocking and IOCP backends.
//! - `device`: `DeviceIoControl`, on the blocking and IOCP backends.
//!
//! The default feature set is empty, keeping the core completion machinery (the
//! raw IOCP and blocking backends, owned endpoints, and pinned operations)
//! minimal. A narrow unsafe submission seam ([`AssociatedEndpoint::submit`] and
//! the [`Operation`] primitives) stays available for families without an adapter.
//! Fully generic, fully safe overlapped submission remains intentionally
//! unsolved; the per-family adapters are the sanctioned safe path.

#![warn(missing_docs)]

#[cfg(windows)]
mod blocking;

#[cfg(windows)]
mod config;

#[cfg(all(windows, feature = "device"))]
mod device;

#[cfg(windows)]
mod endpoint;

#[cfg(all(windows, feature = "fs"))]
mod fs;

#[cfg(windows)]
mod iocp;

#[cfg(windows)]
mod operation;

#[cfg(all(windows, feature = "socket"))]
mod socket;

#[cfg(windows)]
pub use blocking::BlockingEndpoint;

#[cfg(windows)]
pub use config::{SourceTrackingAlreadySet, set_source_tracking, source_tracking_enabled};

#[cfg(all(windows, feature = "device"))]
pub use device::DeviceIoControlIo;

#[cfg(windows)]
pub use endpoint::UnassociatedEndpoint;

#[cfg(all(windows, feature = "fs"))]
pub use fs::{FILE_FLAG_NO_BUFFERING, FileIo, PAGE_SIZE, PageBuffers, ScatterGatherIo};

#[cfg(windows)]
pub use iocp::{AssociatedEndpoint, Completion, CompletionPort, Issued, OperationId, Submitted};

#[cfg(windows)]
pub use operation::{Operation, OperationState, reclaim_overlapped};

#[cfg(all(windows, feature = "socket"))]
pub use socket::{AssociatedSocket, BlockingSocket, SocketIo};
