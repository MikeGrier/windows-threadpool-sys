// Copyright (c) 2026 Mike Grier
//! Safe enumeration of Windows processor, cache, and memory topology.
//!
//! `GetLogicalProcessorInformationEx` is the Win32 entry point for topology.
//! The `windows` crate exposes it as typed but still `unsafe` FFI: a raw
//! output pointer, two-call sizing, and variable-length records that must be
//! walked by their own self-reported `Size` rather than indexed as a slice.
//! Several records also declare a trailing array as length 1 while actually
//! holding as many entries as a separate `GroupCount` field reports -- reading
//! past element 0 is exactly what correct use requires, and exactly what Rust
//! calls undefined behavior if done through the declared type. This crate
//! does that walk once, safely, and hands back owned records.
//!
//! # Scope
//!
//! Safe enumeration, not an opinionated topology model. See `DESIGN-NOTES.md`
//! beside the source for the full reasoning, including a cross-check against
//! Linux's topology model and what this crate deliberately does not attempt.

#![warn(missing_docs)]

#[cfg(windows)]
mod processor_set;
#[cfg(windows)]
mod relation;
#[cfg(windows)]
mod walk;

#[cfg(windows)]
pub use processor_set::ProcessorSet;
#[cfg(windows)]
pub use relation::{
    CacheKind, CacheRelation, CoreRelation, GroupRelation, NumaNodeRelation, PackageRelation,
    Relations, discover,
};
