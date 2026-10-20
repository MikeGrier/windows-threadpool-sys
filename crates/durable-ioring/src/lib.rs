// Copyright (c) 2026 Mike Grier
//! Epoch-based durability for a file-I/O ring, portable: the contract, the core that decides when
//! writes are durable and what a failure puts at risk, the target and backend traits, an in-memory
//! backend, and a conformance oracle. A driver supplies the platform; `win-durable-ioring` is the
//! Windows one, built on `windows-ioring-sys`. What this is, and for whom, is in the crate's
//! readme; what follows is its contract, the specification the code satisfies rather than defines.
//!
#![doc = include_str!("../CONTRACT.md")]
// DI-D-4: no unsafe code, from the first commit. A call no lower crate wraps safely is added to
// that crate, never written here.
#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod access;
pub mod backend;
mod clock;
mod contract;
mod diagnostics;
mod domains;
pub mod driver;
mod durability;
mod engine;
mod error_code;
mod ids;
mod io_core;
mod io_target;
mod key;
pub mod memory;
pub mod oracle;
mod provider;
mod pump;
#[cfg(test)]
mod test_support;
mod types;

pub use access::{
    Access, CanRead, CanWrite, DataAccess, HasData, NoData, ReadOnly, ReadWrite, WriteOnly,
};
pub use clock::{Instant, StampClock};
pub use contract::{
    DurableRing, EntryOf, EpochId, EpochIdOf, FailureIdOf, HandleOf, Identities, Lin, OpIdOf,
    PushResult, RegisteredBufferRing, TokenOf,
};
pub use diagnostics::{
    Diagnostic, DiagnosticKind, DiagnosticSink, FanOutDiagnostics, NoDiagnostics, StderrDiagnostics,
};
pub use error_code::ErrorCode;
pub use ids::{DioringIds, FailureId, FailureToken, Lineage, LineageHandle, OpId};
pub use io_core::{CoreActions, DuplicateDomain, DurableIoCore};
pub use io_target::{
    Capabilities, Geometry, GeometryError, IoTarget, IoTargetDescription, WarrantyClass,
};
pub use key::{Key, ReadKey, TargetKey, WriteKey};
pub use provider::{
    DomainCompletion, DomainOutcome, DomainRequest, DurabilityProvider, FlushRequest,
    IoTargetWrites, ProviderPart, ProviderRequest,
};
pub use pump::Waitable;
pub use types::{
    AddTargetCause, AddTargetError, DurabilityRequest, EndLineageError, EndLineageRefusal, Entry,
    Epoch, EpochState, Failed, FailureCause, FailureInfo, FlushDomain, HeldOperation, ImportScope,
    Imported, IoFault, IoTargetBusy, IoTargetKey, Leftovers, LineageBusy, LineageInfo, Marking,
    MarkingKind, OpCompletion, OpKind, Outcome, PushError, PushRefusal, ReadOptions,
    RemoveTargetError, Resolution, ResolveError, ResolveRefusal, RetireLineageError,
    RetireLineageRefusal, SealError, SuspectSet, SuspectWrite, Tag, UnknownLineage, WriteCaching,
    WriteOptions,
};
// The types the contract's signatures name from the crate it is built on, so a consumer needs no
// direct dependency on it to call it.
pub use io_buffers::{IoBuf, IoBufMut, RegisteredSpan};
