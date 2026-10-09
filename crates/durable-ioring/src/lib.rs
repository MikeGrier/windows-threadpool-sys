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

mod clock;
mod contract;
mod delivery;
mod diagnostics;
mod dioring;
mod error_code;
#[cfg(test)]
mod fake;
mod ids;
pub mod oracle;
mod provider;
mod types;

pub use clock::{Instant, StampClock};
pub use contract::{
    DurableRing, EntryOf, EpochId, EpochIdOf, FailureIdOf, HandleOf, Identities, Lin, OpIdOf,
    PushResult, RegisteredBufferRing, TokenOf,
};
pub use delivery::{DeliveryHandle, DeliverySetupError, EntryDelivery};
pub use diagnostics::{
    Diagnostic, DiagnosticKind, DiagnosticSink, FanOutDiagnostics, NoDiagnostics, StderrDiagnostics,
};
pub use dioring::{Dioring, Setup, SetupError, SetupRefusal, TargetSetup, TimeBase};
pub use error_code::ErrorCode;
pub use ids::{DioringIds, FailureId, FailureToken, Lineage, LineageHandle, OpId};
pub use provider::{
    DomainCompletion, DomainRequest, DurabilityProvider, FlushRequest, IoTargetWrites,
};
pub use types::{
    AddTargetError, DurabilityRequest, EndLineageError, EndLineageRefusal, Entry, Epoch,
    EpochState, Failed, FailureCause, FailureInfo, FileOptions, FlushDomain, HeldOperation,
    ImportScope, Imported, IoFault, IoTargetBusy, IoTargetKey, Leftovers, LineageBusy, LineageInfo,
    Marking, MarkingKind, OpCompletion, OpKind, Outcome, PushError, PushRefusal, ReadOptions,
    RemoveTargetError, Resolution, ResolveError, ResolveRefusal, RetireLineageError,
    RetireLineageRefusal, SealError, SuspectSet, SuspectWrite, Tag, UnknownLineage, WriteCaching,
    WriteOptions,
};
// The types the contract's signatures name from the crates it is built on, so a consumer needs no
// direct dependency on them to call it.
pub use win_shared_os_owned_handle::SharedHandle;
pub use win_sync_sys::Event;
pub use win_time_sys::{InterruptTime, TimePoint};
pub use windows_ioring_sys::{IoBuf, IoBufMut, RegisteredSpan};
