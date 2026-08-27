// Copyright (c) 2026 Mike Grier
//! Windows-only platform layer for asynchronous flat directory enumeration.
//!
//! One request enumerates one directory. The crate owns bounded submission and
//! completion rings, lossless backpressure, cancellation, submitter security
//! context transport, and a caller-buffered `GetFileInformationByHandleEx`
//! engine. Recursive traversal belongs in a separate layer that composes these
//! flat requests.
//!
//! The public API is scheduled by M5 in the workspace checklist and is not yet
//! implemented.

#![cfg(windows)]
#![forbid(unsafe_op_in_unsafe_fn)]
