// Copyright (c) 2026 Mike Grier
//! The crate's error taxonomy.
//!
//! Failures split by *when* they are observable, which is the distinction the
//! settled contract draws. Building a request or a query fails synchronously,
//! on the caller's own thread, before anything has been accepted
//! ([`RequestError`], [`PredicateError`]). Once an enumeration has been
//! accepted it owns a reserved completion slot, so every later failure arrives
//! as one ordered terminal outcome carrying an [`EnumerationError`].
//!
//! Every native failure keeps the raw Win32 code it arrived with. The crate
//! owns the *classification* -- which is why an unsupported directory-
//! information class is its own variant rather than a code a caller has to
//! recognise -- but it never discards the code that classification was derived
//! from.

use std::fmt;
use std::io;

use windows_impersonation_token_sys::{ApplyError, CaptureError, ImpersonationToken};

use crate::request::EnumerationRequest;

/// A raw Win32 error code, kept in the currency it arrived in.
///
/// Every failing API this crate calls is a classic last-error API, so a code is
/// always a `WIN32_ERROR` rather than an `HRESULT`. Keeping the raw value beside
/// the crate's own classification means a caller can act on either.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Win32Error(u32);

impl Win32Error {
    /// Wrap a raw `WIN32_ERROR`.
    #[must_use]
    pub const fn from_code(code: u32) -> Self {
        Self(code)
    }

    /// Take the code from an OS error, or `0` if it carries none.
    ///
    /// A last-error API always sets one, so `0` covers only a fabricated
    /// [`io::Error`] with no OS error behind it.
    #[must_use]
    pub fn from_io(error: &io::Error) -> Self {
        Self(
            error
                .raw_os_error()
                .and_then(|code| u32::try_from(code).ok())
                .unwrap_or(0),
        )
    }

    /// The last error of the calling thread.
    ///
    /// Call immediately after the failing Win32 call, before anything else can
    /// overwrite the thread's last error.
    #[must_use]
    pub(crate) fn last() -> Self {
        Self::from_io(&io::Error::last_os_error())
    }

    /// The raw `WIN32_ERROR` value.
    #[must_use]
    pub const fn code(self) -> u32 {
        self.0
    }

    /// The same failure as a standard [`io::Error`], for callers that funnel
    /// everything through `std::io`.
    #[must_use]
    pub fn to_io_error(self) -> io::Error {
        io::Error::from_raw_os_error(
            i32::try_from(self.0).expect("a WIN32_ERROR always fits in an i32"),
        )
    }
}

impl fmt::Display for Win32Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Win32 error {} ({})", self.0, self.to_io_error())
    }
}

/// Why a request could not be built.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum RequestFailure {
    /// The path had no code units. An empty path names no directory.
    EmptyPath,
    /// The path contained an interior NUL. Win32 would stop at it and open a
    /// different, shorter path than the caller named.
    InteriorNul,
    /// An ordinary path, or the fully qualified form it resolved to, did not fit
    /// the ordinary `MAX_PATH` limit including its terminator.
    ///
    /// This limit is deliberate rather than incidental: it keeps behaviour
    /// independent of the host executable's `longPathAware` manifest. Supply a
    /// fully qualified `\\?\` path to enumerate a longer one.
    PathTooLong,
    /// A `\\?\` path was not fully qualified, so Win32 would not interpret it as
    /// the verbatim absolute path that prefix promises.
    NotFullyQualified,
    /// Windows could not resolve an ordinary path to its fully qualified form.
    PathResolution,
    /// The requested native buffer capacity, after clamping and alignment,
    /// cannot be passed to Win32 as a `u32`.
    BufferCapacityUnrepresentable,
}

impl RequestFailure {
    /// A short description of the failure, without any raw code.
    const fn describe(self) -> &'static str {
        match self {
            RequestFailure::EmptyPath => "the path is empty",
            RequestFailure::InteriorNul => "the path contains an interior NUL",
            RequestFailure::PathTooLong => {
                "the path exceeds MAX_PATH; supply a fully qualified \\\\?\\ path"
            }
            RequestFailure::NotFullyQualified => "the \\\\?\\ path is not fully qualified",
            RequestFailure::PathResolution => "the path could not be resolved",
            RequestFailure::BufferCapacityUnrepresentable => {
                "the native buffer capacity does not fit a Win32 u32"
            }
        }
    }
}

/// A synchronous failure while building an [`EnumerationRequest`].
///
/// [`EnumerationRequest`]: crate::EnumerationRequest
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RequestError {
    failure: RequestFailure,
    code: Option<Win32Error>,
}

impl RequestError {
    pub(crate) const fn new(failure: RequestFailure) -> Self {
        Self {
            failure,
            code: None,
        }
    }

    pub(crate) const fn with_code(failure: RequestFailure, code: Win32Error) -> Self {
        Self {
            failure,
            code: Some(code),
        }
    }

    /// What about the request was rejected.
    #[must_use]
    pub const fn failure(&self) -> RequestFailure {
        self.failure
    }

    /// The raw Win32 code behind the failure, when Windows produced one.
    ///
    /// Only [`RequestFailure::PathResolution`] arises from a Win32 call; the
    /// other failures are decided by this crate before any call is made.
    #[must_use]
    pub const fn code(&self) -> Option<Win32Error> {
        self.code
    }
}

impl fmt::Display for RequestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.code {
            Some(code) => write!(f, "{}: {code}", self.failure.describe()),
            None => f.write_str(self.failure.describe()),
        }
    }
}

impl std::error::Error for RequestError {}

/// Why an enumeration was not admitted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum BeginFailure {
    /// The submission ring had no room for ordinary traffic.
    ///
    /// Reserved cancellation and abandonment messages are unaffected: this is
    /// backpressure on *starting* work, applied where a caller can respond to
    /// it.
    SubmissionRingFull,
    /// The completion ring could not reserve the terminal slot this enumeration
    /// would owe.
    ///
    /// Reservations never take the ring's last slot, so this is reached when the
    /// session is already carrying as many enumerations as its completion ring
    /// can account for.
    CompletionRingFull,
    /// The receiver is gone, so the session no longer starts anything.
    Abandoned,
    /// The caller's security context could not be captured.
    TokenCapture,
}

impl BeginFailure {
    const fn describe(self) -> &'static str {
        match self {
            BeginFailure::SubmissionRingFull => "the submission ring is full",
            BeginFailure::CompletionRingFull => {
                "the completion ring cannot reserve a terminal slot"
            }
            BeginFailure::Abandoned => "the session has been abandoned by its receiver",
            BeginFailure::TokenCapture => "the caller's security context could not be captured",
        }
    }
}

/// A synchronous refusal to start an enumeration.
///
/// The request -- and the captured security context, when there was one -- come
/// back with the error, because nothing was accepted: a caller can retry with
/// exactly what it submitted rather than rebuilding it.
#[derive(Debug)]
pub struct BeginError {
    failure: BeginFailure,
    request: EnumerationRequest,
    token: Option<ImpersonationToken>,
    capture: Option<CaptureError>,
}

impl BeginError {
    pub(crate) fn rejected(
        failure: BeginFailure,
        request: EnumerationRequest,
        token: Option<ImpersonationToken>,
    ) -> Self {
        Self {
            failure,
            request,
            token,
            capture: None,
        }
    }

    pub(crate) fn capture(request: EnumerationRequest, capture: CaptureError) -> Self {
        Self {
            failure: BeginFailure::TokenCapture,
            request,
            token: None,
            capture: Some(capture),
        }
    }

    /// Why the enumeration was refused.
    #[must_use]
    pub const fn failure(&self) -> BeginFailure {
        self.failure
    }

    /// The request that was refused.
    #[must_use]
    pub const fn request(&self) -> &EnumerationRequest {
        &self.request
    }

    /// Take back the request and, when one was captured, the security context,
    /// so a retry costs neither a rebuild nor a second capture.
    #[must_use]
    pub fn into_parts(self) -> (EnumerationRequest, Option<ImpersonationToken>) {
        (self.request, self.token)
    }

    /// The capture failure behind a [`BeginFailure::TokenCapture`].
    #[must_use]
    pub const fn capture_error(&self) -> Option<&CaptureError> {
        self.capture.as_ref()
    }
}

impl fmt::Display for BeginError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.capture {
            Some(capture) => write!(f, "{}: {capture}", self.failure.describe()),
            None => f.write_str(self.failure.describe()),
        }
    }
}

impl std::error::Error for BeginError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.capture
            .as_ref()
            .map(|capture| capture as &(dyn std::error::Error + 'static))
    }
}

/// Why a session could not be built.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum SessionFailure {
    /// The submission ring could not carry one enumeration.
    ///
    /// It needs room for the session's standing abandon message, one
    /// enumeration's reserved cancellation, and one ordinary begin.
    SubmissionCapacityTooSmall,
    /// The completion ring could not carry one enumeration.
    ///
    /// It needs room for one reserved terminal outcome and one entry, and
    /// reservations never take the last slot.
    CompletionCapacityTooSmall,
    /// Windows refused to create the servicer's thread-pool work object.
    WorkObject,
}

impl SessionFailure {
    const fn describe(self) -> &'static str {
        match self {
            SessionFailure::SubmissionCapacityTooSmall => {
                "the submission ring is too small to carry one enumeration"
            }
            SessionFailure::CompletionCapacityTooSmall => {
                "the completion ring is too small to carry one enumeration"
            }
            SessionFailure::WorkObject => "the servicer's work object could not be created",
        }
    }
}

/// A synchronous failure while building a session.
#[derive(Debug)]
pub struct SessionError {
    failure: SessionFailure,
    source: Option<io::Error>,
}

impl SessionError {
    pub(crate) const fn new(failure: SessionFailure) -> Self {
        Self {
            failure,
            source: None,
        }
    }

    pub(crate) const fn with_source(failure: SessionFailure, source: io::Error) -> Self {
        Self {
            failure,
            source: Some(source),
        }
    }

    /// What about the session was rejected.
    #[must_use]
    pub const fn failure(&self) -> SessionFailure {
        self.failure
    }

    /// The OS error behind the failure, when Windows produced one.
    #[must_use]
    pub const fn os_error(&self) -> Option<&io::Error> {
        self.source.as_ref()
    }
}

impl fmt::Display for SessionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.source {
            Some(source) => write!(f, "{}: {source}", self.failure.describe()),
            None => f.write_str(self.failure.describe()),
        }
    }
}

impl std::error::Error for SessionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source
            .as_ref()
            .map(|source| source as &(dyn std::error::Error + 'static))
    }
}

/// Why a query-by-example clause was rejected.
///
/// Both cases describe a clause that would silently match everything. Rejecting
/// them turns a likely caller mistake into a reported error rather than an
/// invisible match-all.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PredicateFailure {
    /// An attribute mask was zero. Every bit of an empty mask is both set and
    /// clear, so the clause is vacuous either way round.
    EmptyAttributeMask,
    /// A name-pattern set was empty. It matches nothing, and its negation
    /// matches everything.
    EmptyNameSet,
}

impl PredicateFailure {
    const fn describe(self) -> &'static str {
        match self {
            PredicateFailure::EmptyAttributeMask => {
                "an attribute mask clause requires a non-zero mask"
            }
            PredicateFailure::EmptyNameSet => "a name-set clause requires at least one pattern",
        }
    }
}

/// A synchronous failure while building a query.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PredicateError {
    failure: PredicateFailure,
}

impl PredicateError {
    pub(crate) const fn new(failure: PredicateFailure) -> Self {
        Self { failure }
    }

    /// What about the clause was rejected.
    #[must_use]
    pub const fn failure(&self) -> PredicateFailure {
        self.failure
    }
}

impl fmt::Display for PredicateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.failure.describe())
    }
}

impl std::error::Error for PredicateError {}

/// Which part of a native directory record failed validation.
///
/// Every variant describes a record the crate refused to read rather than one it
/// read incorrectly: the check happens before the field is touched.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum MalformedRecord {
    /// The record did not start on the alignment its fixed fields require.
    Alignment,
    /// The remaining buffer was too short to hold the record's fixed fields.
    TruncatedFixedFields,
    /// The next-entry offset did not advance within the returned batch.
    NextEntryOffset,
    /// The name length was not a whole number of UTF-16 code units.
    OddNameLength,
    /// The name extended past the end of the returned batch.
    NameOutOfBounds,
    /// A native size field was negative, so it cannot be a byte count.
    NegativeSize,
}

impl MalformedRecord {
    const fn describe(self) -> &'static str {
        match self {
            MalformedRecord::Alignment => "the record is misaligned",
            MalformedRecord::TruncatedFixedFields => "the record's fixed fields are truncated",
            MalformedRecord::NextEntryOffset => "the record's next-entry offset does not advance",
            MalformedRecord::OddNameLength => {
                "the record's name length is not a whole code-unit count"
            }
            MalformedRecord::NameOutOfBounds => "the record's name extends past the batch",
            MalformedRecord::NegativeSize => "the record reports a negative size",
        }
    }
}

/// Why an accepted enumeration failed.
///
/// An enumeration reaches this only after it has been accepted, so every value
/// here arrives as the [`Failed`](crate::TerminalOutcome::Failed) terminal
/// outcome for one [`EnumerationId`](crate::EnumerationId) -- never as the
/// result of a submission call.
///
/// Clean exhaustion is deliberately absent. `ERROR_NO_MORE_FILES` from any
/// refill, and `ERROR_FILE_NOT_FOUND` from the very first one, are the two forms
/// of "this directory has no more entries" and produce
/// [`Completed`](crate::TerminalOutcome::Completed).
#[derive(Debug)]
#[non_exhaustive]
pub enum EnumerationError {
    /// The worker could not apply the submitted impersonation context, so the
    /// directory was never opened under the submitter's identity.
    Impersonation(ApplyError),
    /// The directory could not be opened. Existence, access, and
    /// not-a-directory failures all arrive here, distinguished by the raw code.
    DirectoryOpen(Win32Error),
    /// A volume serial was [`Required`](crate::FileIdentityMode::Required) and
    /// could not be obtained, so no entry could carry the globally meaningful
    /// identity the request demanded.
    VolumeIdentity(Win32Error),
    /// The filesystem does not support extended directory information.
    ///
    /// The crate does not fall back to a metadata-poorer enumeration API,
    /// because that would silently drop change time, allocation size,
    /// extended-attribute size, and the 128-bit file ID from the contract.
    UnsupportedExtendedDirectoryInfo(Win32Error),
    /// A directory-information query failed for a reason that is neither clean
    /// exhaustion, an unsupported class, nor an oversize record.
    DirectoryQuery(Win32Error),
    /// One record did not fit the request's fixed native buffer.
    ///
    /// The buffer never grows, so this is reported rather than hidden. Retry
    /// with an explicitly larger capacity.
    RecordTooLarge {
        /// The effective capacity, in bytes, that the record did not fit.
        buffer_capacity: usize,
        /// The raw code the failing refill reported.
        code: Win32Error,
    },
    /// A returned record failed validation before any of its fields were read.
    MalformedRecord(MalformedRecord),
}

impl EnumerationError {
    /// The raw Win32 code behind this failure, when one is available.
    ///
    /// [`MalformedRecord`](Self::MalformedRecord) has none -- the record was
    /// rejected by this crate, not by Windows -- and
    /// [`Impersonation`](Self::Impersonation) carries the sibling crate's typed
    /// error, whose own code is reachable through it.
    #[must_use]
    pub fn code(&self) -> Option<Win32Error> {
        match self {
            EnumerationError::Impersonation(error) => error
                .raw_os_error()
                .and_then(|code| u32::try_from(code).ok())
                .map(Win32Error::from_code),
            EnumerationError::DirectoryOpen(code)
            | EnumerationError::VolumeIdentity(code)
            | EnumerationError::UnsupportedExtendedDirectoryInfo(code)
            | EnumerationError::DirectoryQuery(code)
            | EnumerationError::RecordTooLarge { code, .. } => Some(*code),
            EnumerationError::MalformedRecord(_) => None,
        }
    }
}

impl fmt::Display for EnumerationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EnumerationError::Impersonation(error) => {
                write!(f, "the submitted impersonation context failed: {error}")
            }
            EnumerationError::DirectoryOpen(code) => {
                write!(f, "the directory could not be opened: {code}")
            }
            EnumerationError::VolumeIdentity(code) => {
                write!(f, "the required volume identity is unavailable: {code}")
            }
            EnumerationError::UnsupportedExtendedDirectoryInfo(code) => write!(
                f,
                "extended directory information is unsupported here: {code}"
            ),
            EnumerationError::DirectoryQuery(code) => {
                write!(f, "the directory query failed: {code}")
            }
            EnumerationError::RecordTooLarge {
                buffer_capacity,
                code,
            } => write!(
                f,
                "one record exceeds the {buffer_capacity}-byte native buffer: {code}"
            ),
            EnumerationError::MalformedRecord(detail) => {
                write!(
                    f,
                    "a native record failed validation: {}",
                    detail.describe()
                )
            }
        }
    }
}

impl std::error::Error for EnumerationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            EnumerationError::Impersonation(error) => Some(error),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests;
