// Copyright (c) 2026 Mike Grier
//! Windows-only platform layer for asynchronous flat directory enumeration.
//!
//! One request enumerates one directory. The crate owns bounded submission and
//! completion rings, lossless backpressure, cancellation, submitter security
//! context transport, and a caller-buffered `GetFileInformationByHandleEx`
//! engine. Recursive traversal belongs in a separate layer that composes these
//! flat requests.
//!
//! # Native values stay native
//!
//! Names and paths are native-width WTF-16 ([`wtf_string`]), so an ill-formed
//! surrogate a filesystem contains survives the round trip. Times are signed
//! Windows tick counts ([`WindowsFileTimestamp`]), attributes are the raw
//! `FILE_ATTRIBUTE_*` bitmask, and a file ID keeps the record's exact 16 bytes.
//! Nothing is converted eagerly into a portable shape whose losses a caller
//! could not undo.
//!
//! # Safety
//!
//! The public surface is entirely safe: every FFI call the native engine makes
//! is confined to a single caller-owned, size-checked buffer
//! ([`EnumerationRequest::with_buffer_capacity`]), and no directory entry is
//! ever opened individually -- the engine reads only from the batched
//! `GetFileInformationByHandleEx` listing of the one directory handle the
//! request named. A submitted enumeration's security context is captured
//! synchronously on the submitter's own thread, before the request becomes
//! visible to any worker, so the later directory open always runs as whoever
//! asked for it rather than as the pool. The unsafe internals that make this
//! true -- buffer aliasing, handle ownership, and thread-pool callback
//! lifetime -- are recorded in [DESIGN-NOTES.md][1] and [DESIGN-RATIONALE.md][2].
//!
//! [1]: https://github.com/MikeGrier/windows-threadpool-sys/blob/main/crates/windows-file-enumeration-sys/DESIGN-NOTES.md
//! [2]: https://github.com/MikeGrier/windows-threadpool-sys/blob/main/crates/windows-file-enumeration-sys/DESIGN-RATIONALE.md
//!
//! # Building a predicate
//!
//! Build a request for one directory, delivering only files larger than 4 KiB
//! whose names end in `.log`:
//!
//! ```no_run
//! use windows_file_enumeration_sys::{
//!     ComparisonOperator, EntryType, EnumerationRequest, NamePattern, PatternToken,
//!     PredicateClause, QueryByExample,
//! };
//! use wtf_string::Wtf16String;
//!
//! let suffix = NamePattern::empty()
//!     .with(PatternToken::AnyRun)
//!     .with(PatternToken::Literal(Wtf16String::from(".log")));
//!
//! let query = QueryByExample::new()
//!     .with(PredicateClause::Name {
//!         pattern: suffix,
//!         case: Default::default(),
//!         negated: false,
//!     })?
//!     .with(PredicateClause::IsType {
//!         entry_type: EntryType::File,
//!         negated: false,
//!     })?
//!     .with(PredicateClause::LogicalSize {
//!         operator: ComparisonOperator::Greater,
//!         value: 4096,
//!     })?;
//!
//! let request = EnumerationRequest::for_path("C:/logs".as_ref())?.with_predicate(query);
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! # Running an enumeration
//!
//! [`Session::new`] returns a producing [`Session`] and its single [`Receiver`].
//! [`Session::try_begin`] captures the caller's own security context and starts
//! the enumeration; entries and exactly one [`Completion::Terminal`] arrive on
//! the receiver, every entry of one enumeration before its terminal:
//!
//! ```no_run
//! use windows_file_enumeration_sys::{Completion, EnumerationRequest, Session};
//!
//! let (session, receiver) = Session::new(8, 8)?;
//! let request = EnumerationRequest::for_path("C:/logs".as_ref())?;
//! session.try_begin(request)?.detach();
//!
//! while let Some(completion) = receiver.recv() {
//!     match completion {
//!         Completion::Entry { entry, .. } => println!("{}", entry.name()),
//!         Completion::Terminal { outcome, .. } => {
//!             println!("finished: {outcome:?}");
//!             break;
//!         }
//!     }
//! }
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! # Traversal-style submission
//!
//! A recursive traversal layer captures one security context and reuses it for
//! every directory in the tree with [`Session::try_begin_with_token`], instead
//! of paying a fresh capture per directory on whatever thread happens to be
//! submitting:
//!
//! ```no_run
//! use windows_file_enumeration_sys::{EnumerationRequest, Session};
//! use windows_impersonation_token_sys::ImpersonationToken;
//!
//! let (session, receiver) = Session::new(8, 8)?;
//! let token = ImpersonationToken::capture()?;
//!
//! for directory in ["C:/logs", "C:/logs/archive"] {
//!     let request = EnumerationRequest::for_path(directory.as_ref())?;
//!     session
//!         .try_begin_with_token(request, token.clone())?
//!         .detach();
//! }
//! # drop(receiver);
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

#![cfg(windows)]
#![forbid(unsafe_op_in_unsafe_fn)]
#![warn(missing_docs)]

mod admission;
mod buffer;
mod completion;
mod completion_ring;
mod engine;
mod entry;
mod error;
mod native;
mod path;
mod pattern;
mod predicate;
mod record;
mod registry;
mod request;
mod session;
mod submission_ring;
mod timestamp;

#[cfg(test)]
mod model;
#[cfg(test)]
mod scratch;
#[cfg(test)]
mod testing;

pub use admission::{EnumerationHandle, TokenCaptureError};
pub use completion::{Completion, EnumerationId, TerminalOutcome};
pub use entry::{DirectoryEntry, EntryType, FileIdentity, FileIdentityMode};
pub use error::{
    BeginError, BeginFailure, EnumerationError, MalformedRecord, PredicateError, PredicateFailure,
    RequestError, RequestFailure, SessionError, SessionFailure, Win32Error,
};
pub use pattern::{CaseSensitivity, NamePattern, PatternToken};
pub use predicate::{
    ComparisonOperator, EntryPredicate, PredicateClause, QueryByExample, TimestampField,
};
pub use request::{DEFAULT_BUFFER_CAPACITY, EnumerationRequest, MINIMUM_BUFFER_CAPACITY};
pub use session::{
    MINIMUM_COMPLETION_RING_CAPACITY, MINIMUM_SUBMISSION_CAPACITY, Receiver, Session,
};
pub use timestamp::WindowsFileTimestamp;
