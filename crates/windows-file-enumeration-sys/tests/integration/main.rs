// Copyright (c) 2026 Mike Grier
//! The real-Windows integration suite (FE-13).
//!
//! Everything here runs against real directories, real files, and (except
//! where a test says otherwise) the crate's live thread pool, because that is
//! the one thing the crate's unit tests -- which deliberately keep the
//! filesystem side small and fast -- cannot exercise on their own. See each
//! submodule's own doc comment for what it covers.

mod support;

mod cancellation;
mod capability;
mod directories;
mod metadata;
mod paths;
mod predicates;
mod reparse;
mod scale;
