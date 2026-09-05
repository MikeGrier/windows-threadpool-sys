// Copyright (c) 2026 Mike Grier
//! Owned overlapped I/O endpoints and pinned operations for Windows.
//!
//! This crate provides the ownership, association, completion, cancellation, and
//! rundown model for overlapped I/O on top of `windows-sys`. It is the reusable
//! foundation beneath `windows-threadpool-sys`: raw I/O completion ports and the
//! object-based thread pool share endpoint and operation storage while remaining
//! distinct completion backends.
//!
//! # Operation-family adapters
//!
//! Endpoints are created safely with [`UnassociatedEndpoint::open`], and each
//! operation family has an adapter behind an opt-in feature. The `fs` and
//! `socket` adapters are fully safe; the `device` adapter owns its buffers but
//! its `ioctl` is `unsafe`, because an arbitrary control code may embed pointers
//! to buffers the adapter cannot own:
//!
//! - `fs`: file read/write and scatter/gather, on the blocking and IOCP backends.
//!   Fully safe.
//! - `socket`: socket send/receive, on the blocking and IOCP backends. Fully
//!   safe.
//! - `device`: `DeviceIoControl` on both backends, through a buffer-owning but
//!   `unsafe` raw-control-code seam.
//!
//! The default feature set is empty, keeping the core completion machinery (the
//! raw IOCP and blocking backends, owned endpoints, and pinned operations)
//! minimal. A narrow unsafe submission seam ([`AssociatedEndpoint::submit`] and
//! the [`Operation`] primitives) stays available for families without an adapter.
//! Fully generic, fully safe overlapped submission remains intentionally
//! unsolved; the per-family adapters are the sanctioned safe path.
//!
//! # Operation identity
//!
//! Submitting returns an [`OperationId`] that names that operation for the life
//! of the process, not merely while its storage address stays put. Cancelling
//! validates the identity against the backend's live operations first, so an
//! identity kept past its operation's completion is rejected rather than applied
//! to a later operation that reused the same storage. Holding an identity too
//! long is therefore safe, and cancellation races safely against completion.

#![warn(missing_docs)]

#[cfg(windows)]
mod blocking;

#[cfg(windows)]
mod config;

#[cfg(windows)]
mod buf;

#[cfg(all(windows, feature = "device"))]
mod device;

#[cfg(windows)]
mod endpoint;

#[cfg(all(windows, feature = "fs"))]
mod fs;

#[cfg(windows)]
mod identity;

#[cfg(windows)]
mod iocp;

#[cfg(windows)]
mod operation;

#[cfg(all(windows, feature = "socket"))]
mod socket;

#[cfg(windows)]
mod started;

#[cfg(windows)]
pub use blocking::{BlockingEndpoint, TryFromEndpointError};

#[cfg(windows)]
pub use buf::{IoBuf, IoBufMut};

#[cfg(windows)]
pub use config::{SourceTrackingAlreadySet, set_source_tracking, source_tracking_enabled};

#[cfg(all(windows, feature = "device"))]
pub use device::DeviceIoControlIo;

#[cfg(windows)]
pub use endpoint::{NotificationModes, UnassociatedEndpoint};

#[cfg(all(windows, feature = "fs"))]
pub use fs::{FILE_FLAG_NO_BUFFERING, FileIo, PAGE_SIZE, PageBuffers, ScatterGatherIo};

#[cfg(windows)]
pub use identity::{OperationId, OperationRegistry};

#[cfg(windows)]
pub use iocp::{AssociatedEndpoint, Completion, CompletionPort, Issued, Submitted};

#[cfg(windows)]
pub use operation::{Operation, OperationState, reclaim_overlapped};

#[cfg(all(windows, feature = "socket"))]
pub use socket::{AssociatedSocket, BlockingSocket, SocketIo};

#[cfg(windows)]
pub use started::Started;

// The crate's markdown documentation is compiled as doctests, so an example that
// a contract change invalidates breaks the build instead of quietly teaching the
// old answer. `cfg(doctest)` means these items exist only while rustdoc collects
// tests, so they cost an ordinary build nothing.
// Gated on `fs` because the README's example uses the `fs` adapter's
// `BlockingEndpoint::read`, which does not exist in the default feature set
// (`default = []`). Without the gate the example fails to compile for a reason
// the README already states -- so the gate matches the doctest to what the
// prose says it needs, rather than weakening the example. docs.rs builds with
// `all-features`, so the published documentation is the configuration that
// checks it.
#[cfg(all(doctest, windows, feature = "fs"))]
#[doc = include_str!("../README.md")]
struct ReadmeDoctests;
