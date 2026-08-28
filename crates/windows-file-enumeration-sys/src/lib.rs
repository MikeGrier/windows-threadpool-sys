// Copyright (c) 2026 Mike Grier
//! Windows-only platform layer for asynchronous flat directory enumeration.
//!
//! One request enumerates one directory. The crate owns bounded submission and
//! completion rings, lossless backpressure, cancellation, submitter security
//! context transport, and a caller-buffered `GetFileInformationByHandleEx`
//! engine. Recursive traversal belongs in a separate layer that composes these
//! flat requests.
//!
//! # What is implemented so far
//!
//! The public value types are complete: [`EnumerationRequest`], the
//! [`EntryPredicate`] family, [`DirectoryEntry`] and its metadata, the error
//! taxonomy, and the [`Completion`] records a receiver observes. The session
//! that carries them and the native engine that fills them are scheduled by M5
//! and M6 in the workspace checklist.
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
//! # Example
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
