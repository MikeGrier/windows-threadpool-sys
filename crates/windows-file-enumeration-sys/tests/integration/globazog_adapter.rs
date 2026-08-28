// Copyright (c) 2026 Mike Grier
//! Discharges the D-15 Globazog acceptance gate (FE-14): a real adapter
//! demonstration that `windows_file_enumeration_sys` can replace Globazog's
//! Windows one-directory enumeration backend.
//!
//! # Why this is a reconstruction, not a dependency
//!
//! Globazog (`MikeGrier/globazog-rs`) is a separate repository and is not,
//! and should not become, a dependency of this crate or this test suite --
//! this workspace's only job is to *supply* a layer Globazog can adopt, not
//! to depend on it. [`types`] and [`predicate_types`] therefore reconstruct
//! the exact shapes Globazog's real Windows backend
//! (`crates/globazog/src/sys/win.rs`) and predicate vocabulary
//! (`crates/globazog/src/predicate.rs`) are built from, pinned to commit
//! `55a0b1aec7a93051a675852636ab41a6437440fb`. [`translate`] and [`adapter`]
//! then prove that everything on the other side of that boundary --
//! metadata, predicates, and the error-plus-partial-listing terminal shape
//! -- carries across without loss, using only `windows_file_enumeration_sys`'s
//! public API.
//!
//! # Scope and its explicit limits
//!
//! - **Depth.** `Leaf::Depth` is not reconstructed: it is a property of
//!   Globazog's own recursive-traversal engine composing many single-
//!   directory requests, never a property one directory's own listing can
//!   answer. See [`predicate_types`]'s module doc comment.
//! - **A live mid-stream failure.** Producing a real
//!   `TerminalOutcome::Failed` *after* some entries have already been
//!   delivered needs a filesystem or redirector fault this environment
//!   cannot manufacture on demand (the same gap `capability.rs` documents
//!   for FE-13). [`adapter::finish_scan`] is deliberately a pure function of
//!   an entry list and an outcome for exactly this reason: `tests_errors.rs`
//!   proves the translation -- `Failed` with an empty listing becomes `Err`,
//!   `Failed` with a non-empty one becomes `Ok` plus one `EntryFailure` --
//!   by constructing both cases directly, which needs no live failure to
//!   exercise the property this adapter actually owns.
//! - **No per-entry opens.** Structurally inherited from D-3 (every field
//!   comes from the one batched record the directory listing itself
//!   returns), and checked empirically in `tests_no_per_entry_open.rs`
//!   against a directory junction whose target does not exist.

pub mod adapter;
pub mod predicate_types;
pub mod translate;
pub mod types;

mod tests_errors;
mod tests_metadata;
mod tests_no_per_entry_open;
mod tests_predicates;
mod tests_support;
