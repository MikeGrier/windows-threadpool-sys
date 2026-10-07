// Copyright (c) 2026 Mike Grier
//! A ring for Windows file I/O with durability scheduled into its work, built on
//! `windows-ioring-sys`. What it is, and for whom, is in the crate's readme; what follows is its
//! contract, the specification the code satisfies rather than defines.
//!
#![doc = include_str!("../CONTRACT.md")]
#![cfg(windows)]
// DI-D-4: no unsafe code, from the first commit. A Win32 call no lower crate wraps safely is added
// to that crate, never written here.
#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod contract;
mod dioring;
mod ids;
mod provider;
mod types;

pub use contract::{
    DurableRing, EntryOf, EpochId, EpochIdOf, FailureIdOf, Identities, Lin, OpIdOf, PushResult,
    TokenOf,
};
pub use dioring::{Dioring, FileSetup, Setup, SetupError, SetupRefusal};
pub use ids::{DioringIds, FailureId, FailureToken, Lineage, OpId};
pub use provider::{DomainCompletion, DomainRequest, DurabilityProvider, FileWrites, FlushRequest};
pub use types::{
    AddFileError, Cause, DurabilityRequest, EndLineageError, Entry, Epoch, EpochState, Failed,
    FailureInfo, FileBusy, FileKey, FileOptions, FlushDomain, HeldOperation, ImportScope,
    Leftovers, LineageBusy, LineageInfo, OpCompletion, OpKind, Outcome, PushError, PushRefusal,
    ReadOptions, RemoveFileError, Resolution, ResolveError, ResolveRefusal, RetireLineageError,
    SuspectSet, SuspectWrite, WriteCaching, WriteOptions,
};
