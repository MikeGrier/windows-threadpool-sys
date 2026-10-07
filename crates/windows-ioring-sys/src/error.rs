// Copyright (c) 2026 Mike Grier
//! IoRing errors: `HRESULT`, not the `GetLastError` convention.

use std::fmt;
use std::io;

use windows_sys::Win32::Foundation::{
    IORING_E_COMPLETION_QUEUE_TOO_BIG, IORING_E_COMPLETION_QUEUE_TOO_FULL, IORING_E_CORRUPT,
    IORING_E_REQUIRED_FLAG_NOT_SUPPORTED, IORING_E_SUBMISSION_QUEUE_FULL,
    IORING_E_SUBMISSION_QUEUE_TOO_BIG, IORING_E_SUBMIT_IN_PROGRESS, IORING_E_VERSION_NOT_SUPPORTED,
};
use windows_sys::core::HRESULT;

mod sealed {
    pub trait Sealed {}
    impl Sealed for std::io::Error {}
}

/// A named `IORING_E_*` condition (M10.5).
///
/// The `HRESULT` -> name mapping is defined here once and everywhere else asks
/// (`IoRingError::name` is derived from this, not a second copy of it), so a
/// new condition cannot be added to one and forgotten in the other.
///
/// `#[non_exhaustive]`: the kernel's error set can grow, and a consumer must
/// not be able to write an exhaustive `match` that a new variant would break
/// -- the same reasoning as [`crate::Op`]'s (D-7).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum RingCondition {
    /// `IORING_E_REQUIRED_FLAG_NOT_SUPPORTED`.
    RequiredFlagNotSupported,
    /// `IORING_E_VERSION_NOT_SUPPORTED`.
    VersionNotSupported,
    /// `IORING_E_SUBMISSION_QUEUE_FULL`: the submission queue has no room.
    /// Recoverable -- submit what is queued, drain completions, retry.
    SubmissionQueueFull,
    /// `IORING_E_SUBMISSION_QUEUE_TOO_BIG`.
    SubmissionQueueTooBig,
    /// `IORING_E_COMPLETION_QUEUE_TOO_BIG`.
    CompletionQueueTooBig,
    /// `IORING_E_CORRUPT`: the ring's shared state is inconsistent. Fatal.
    Corrupt,
    /// `IORING_E_SUBMIT_IN_PROGRESS`: another submit is already running on
    /// this ring.
    SubmitInProgress,
    /// `IORING_E_COMPLETION_QUEUE_TOO_FULL`: the completion queue has no room
    /// for more results. Recoverable -- drain completions.
    CompletionQueueTooFull,
}

impl RingCondition {
    /// The condition `code` names, or `None` if this crate does not name it.
    #[must_use]
    pub fn from_hresult(code: HRESULT) -> Option<Self> {
        match code {
            IORING_E_REQUIRED_FLAG_NOT_SUPPORTED => Some(Self::RequiredFlagNotSupported),
            IORING_E_VERSION_NOT_SUPPORTED => Some(Self::VersionNotSupported),
            IORING_E_SUBMISSION_QUEUE_FULL => Some(Self::SubmissionQueueFull),
            IORING_E_SUBMISSION_QUEUE_TOO_BIG => Some(Self::SubmissionQueueTooBig),
            IORING_E_COMPLETION_QUEUE_TOO_BIG => Some(Self::CompletionQueueTooBig),
            IORING_E_CORRUPT => Some(Self::Corrupt),
            IORING_E_SUBMIT_IN_PROGRESS => Some(Self::SubmitInProgress),
            IORING_E_COMPLETION_QUEUE_TOO_FULL => Some(Self::CompletionQueueTooFull),
            _ => None,
        }
    }

    /// The `IORING_E_*` constant's spelling.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::RequiredFlagNotSupported => "IORING_E_REQUIRED_FLAG_NOT_SUPPORTED",
            Self::VersionNotSupported => "IORING_E_VERSION_NOT_SUPPORTED",
            Self::SubmissionQueueFull => "IORING_E_SUBMISSION_QUEUE_FULL",
            Self::SubmissionQueueTooBig => "IORING_E_SUBMISSION_QUEUE_TOO_BIG",
            Self::CompletionQueueTooBig => "IORING_E_COMPLETION_QUEUE_TOO_BIG",
            Self::Corrupt => "IORING_E_CORRUPT",
            Self::SubmitInProgress => "IORING_E_SUBMIT_IN_PROGRESS",
            Self::CompletionQueueTooFull => "IORING_E_COMPLETION_QUEUE_TOO_FULL",
        }
    }

    /// The raw `HRESULT` this condition is reported as.
    #[must_use]
    pub fn code(self) -> HRESULT {
        match self {
            Self::RequiredFlagNotSupported => IORING_E_REQUIRED_FLAG_NOT_SUPPORTED,
            Self::VersionNotSupported => IORING_E_VERSION_NOT_SUPPORTED,
            Self::SubmissionQueueFull => IORING_E_SUBMISSION_QUEUE_FULL,
            Self::SubmissionQueueTooBig => IORING_E_SUBMISSION_QUEUE_TOO_BIG,
            Self::CompletionQueueTooBig => IORING_E_COMPLETION_QUEUE_TOO_BIG,
            Self::Corrupt => IORING_E_CORRUPT,
            Self::SubmitInProgress => IORING_E_SUBMIT_IN_PROGRESS,
            Self::CompletionQueueTooFull => IORING_E_COMPLETION_QUEUE_TOO_FULL,
        }
    }
}

/// An error from an `IoRing` API call.
///
/// Every `IoRing` entry point reports failure as an `HRESULT`, not through
/// `GetLastError` the way most of Win32 does, so this crate cannot reuse
/// `io::Error::last_os_error` the way `windows-overlapped-io-sys` does.
///
/// # Matching on one of these
///
/// A kernel-reported failure always reaches a caller as an [`io::Error`] whose
/// [`kind`](io::Error::kind) is [`io::ErrorKind::Other`], because there is no
/// faithful `ErrorKind` for most `IORING_E_*` conditions (M10.2, D-30). The
/// `HRESULT` is not lost -- it is this value, behind a downcast -- but
/// `kind()` will not find it. `kind()` discriminates only this crate's *own*
/// rejections, which use `Unsupported`, `InvalidInput`, and `AlreadyExists`.
///
/// So a consumer branching on a ring condition -- most often
/// `IORING_E_SUBMISSION_QUEUE_FULL`, the backpressure signal every push's
/// docs name -- asks through [`IoRingErrorExt`] (M10.5), which is the
/// downcast plus the comparison named once rather than hand-rolled:
///
/// ```
/// use windows_ioring_sys::IoRingErrorExt;
///
/// fn classify(error: &std::io::Error) -> &'static str {
///     if error.is_submission_queue_full() {
///         "backpressure: submit, drain, retry"
///     } else if let Some(condition) = error.ring_condition() {
///         condition.name()
///     } else {
///         "not a ring condition"
///     }
/// }
/// ```
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct IoRingError {
    code: HRESULT,
}

impl IoRingError {
    pub(crate) fn new(code: HRESULT) -> Self {
        Self { code }
    }

    /// The raw `HRESULT`.
    #[must_use]
    pub fn code(&self) -> HRESULT {
        self.code
    }

    /// The named `IORING_E_*` constant this value matches, if any.
    ///
    /// Derived from [`IoRingError::condition`] rather than matching the
    /// `HRESULT` a second time, so the two can never disagree about which
    /// codes are named (M10.5).
    #[must_use]
    pub fn name(&self) -> Option<&'static str> {
        self.condition().map(RingCondition::name)
    }

    /// The named condition this error reports, or `None` for an `HRESULT`
    /// this crate does not name (M10.5).
    ///
    /// `match` on this when a consumer needs to distinguish several
    /// conditions; use a predicate like
    /// [`IoRingError::is_submission_queue_full`] for a single branch.
    #[must_use]
    pub fn condition(&self) -> Option<RingCondition> {
        RingCondition::from_hresult(self.code)
    }

    /// Whether this is `IORING_E_SUBMISSION_QUEUE_FULL`: the backpressure
    /// signal every push's documentation names (M3.3).
    ///
    /// Recoverable, and the one condition a normal submission loop is
    /// expected to branch on: submit what is already queued, drain
    /// completions, and retry the push.
    #[must_use]
    pub fn is_submission_queue_full(&self) -> bool {
        self.condition() == Some(RingCondition::SubmissionQueueFull)
    }

    /// Whether this is `IORING_E_COMPLETION_QUEUE_TOO_FULL`: the completion
    /// queue has no room for further results, so completions must be drained
    /// with [`crate::IoRing::try_pop`].
    #[must_use]
    pub fn is_completion_queue_too_full(&self) -> bool {
        self.condition() == Some(RingCondition::CompletionQueueTooFull)
    }

    /// Whether this is `IORING_E_SUBMIT_IN_PROGRESS`: another submit is
    /// already running on this ring.
    #[must_use]
    pub fn is_submit_in_progress(&self) -> bool {
        self.condition() == Some(RingCondition::SubmitInProgress)
    }
}

/// Ask an [`io::Error`] this crate produced about the ring condition inside
/// it (M10.5, D-30).
///
/// Exists because [`io::Error::kind`] cannot answer: every kernel-reported
/// failure is `io::ErrorKind::Other`, since there is no faithful `ErrorKind`
/// for most `IORING_E_*` conditions and inventing one would trade an honest
/// `Other` for a lossy guess. The `HRESULT` survives behind a downcast, and
/// this trait is that downcast plus the comparison, named once here instead
/// of hand-rolled at each call site.
///
/// ```no_run
/// use windows_ioring_sys::{Batch, IoRing, IoRingErrorExt, PushOptions, SharedFile};
///
/// # fn demo(ring: &mut IoRing<Vec<u8>>, file: &SharedFile) -> std::io::Result<()> {
/// let mut batch = Batch::new(ring);
/// match batch.read_owned(file, vec![0_u8; 4096], (), 0, PushOptions::new()) {
///     Ok(_id) => {}
///     // Backpressure, not a failure: submit and drain, then retry.
///     Err(refused) if refused.error.is_submission_queue_full() => {
///         batch.submit()?;
///         return Ok(());
///     }
///     Err(refused) => return Err(refused.into()),
/// }
/// # Ok(())
/// # }
/// ```
///
/// **Sealed**: implemented for [`io::Error`] and nothing else.
pub trait IoRingErrorExt: sealed::Sealed {
    /// The [`IoRingError`] this error wraps, if it came from this crate's
    /// report of a native `HRESULT`.
    ///
    /// `None` for this crate's *own* rejections (`Unsupported`,
    /// `InvalidInput`, `AlreadyExists`), which carry a meaningful
    /// [`io::Error::kind`] instead and never wrap an `HRESULT`.
    fn as_ioring_error(&self) -> Option<&IoRingError>;

    /// The named condition this error reports, if any.
    fn ring_condition(&self) -> Option<RingCondition>;

    /// As [`IoRingError::is_submission_queue_full`].
    fn is_submission_queue_full(&self) -> bool;

    /// As [`IoRingError::is_completion_queue_too_full`].
    fn is_completion_queue_too_full(&self) -> bool;

    /// As [`IoRingError::is_submit_in_progress`].
    fn is_submit_in_progress(&self) -> bool;
}

impl IoRingErrorExt for io::Error {
    fn as_ioring_error(&self) -> Option<&IoRingError> {
        self.get_ref()?.downcast_ref::<IoRingError>()
    }

    fn ring_condition(&self) -> Option<RingCondition> {
        self.as_ioring_error()?.condition()
    }

    fn is_submission_queue_full(&self) -> bool {
        self.as_ioring_error()
            .is_some_and(IoRingError::is_submission_queue_full)
    }

    fn is_completion_queue_too_full(&self) -> bool {
        self.as_ioring_error()
            .is_some_and(IoRingError::is_completion_queue_too_full)
    }

    fn is_submit_in_progress(&self) -> bool {
        self.as_ioring_error()
            .is_some_and(IoRingError::is_submit_in_progress)
    }
}

impl fmt::Debug for IoRingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IoRingError")
            .field("code", &format_args!("0x{:08X}", self.code as u32))
            .finish()
    }
}

impl fmt::Display for IoRingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.name() {
            Some(name) => write!(f, "{name} (HRESULT 0x{:08X})", self.code as u32),
            None => write!(f, "HRESULT 0x{:08X}", self.code as u32),
        }
    }
}

impl std::error::Error for IoRingError {}

/// A push the ring refused, carrying everything the push was given (M30.1,
/// [D-80](../DESIGN-NOTES.md#d-80)).
///
/// Every owned push returns this on refusal: the [`io::Error`] that explains
/// it, the payload the push took (`None` for one that takes no payload, such
/// as a flush, a cancel or a registered-span push), and the sidecar `extra`.
/// [`Batch::register_buffers`](crate::Batch::register_buffers) returns one too,
/// with the whole `Vec` of buffers as its payload and `()` as its sidecar.
/// A refused push queued nothing, so nothing will ever complete to hand these
/// back; this is the only way back.
///
/// The commonest refusal is `IORING_E_SUBMISSION_QUEUE_FULL`. No caller can
/// rule it out beforehand, because building the operation is the test, and it
/// is exactly the case where the caller wants the same buffer for a retry:
///
/// ```no_run
/// use windows_ioring_sys::{Batch, IoRing, IoRingErrorExt, PushOptions, SharedFile};
///
/// # fn demo(ring: &mut IoRing<Vec<u8>>, file: &SharedFile) -> std::io::Result<()> {
/// let mut batch = Batch::new(ring);
/// if let Err(refused) = batch.read_owned(file, vec![0_u8; 4096], (), 0, PushOptions::new()) {
///     if !refused.error.is_submission_queue_full() {
///         return Err(refused.into());
///     }
///     // Backpressure, not a failure: submit what is queued, then retry with
///     // the buffer the refusal handed back.
///     batch.submit()?;
///     let buffer = refused.payload.expect("a read takes a payload");
///     let mut batch = Batch::new(ring);
///     batch.read_owned(file, buffer, (), 0, PushOptions::new())?;
/// }
/// # Ok(())
/// # }
/// ```
///
/// A caller that wants only the error converts with `?` or `.into()`, which
/// keeps the error unchanged (so [`IoRingErrorExt`] still finds the
/// `HRESULT`) and drops the payload and sidecar there, by the caller's
/// choice rather than the ring's.
///
/// The ring-owned parts of an operation are not returned: a file guard and a
/// registered-buffer use belong to the ring, never to the caller, and are
/// released.
///
/// `Debug` and `Display` are written by hand so they place no bound on `T`
/// or `X`. A consumer's buffer type need not be `Debug` for `.expect(..)` to
/// compile.
#[non_exhaustive]
pub struct PushRefused<T, X> {
    /// Why the push was refused.
    pub error: io::Error,
    /// The payload the push took, or `None` if it took none.
    pub payload: Option<T>,
    /// The sidecar the push took.
    pub extra: X,
}

impl<T, X> PushRefused<T, X> {
    pub(crate) fn new(error: io::Error, payload: Option<T>, extra: X) -> Self {
        Self {
            error,
            payload,
            extra,
        }
    }

    /// Discard the payload and sidecar, keeping only the error.
    #[must_use]
    pub fn into_error(self) -> io::Error {
        self.error
    }
}

impl<T, X> From<PushRefused<T, X>> for io::Error {
    fn from(refused: PushRefused<T, X>) -> Self {
        refused.error
    }
}

impl<T, X> fmt::Debug for PushRefused<T, X> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PushRefused")
            .field("error", &self.error)
            .field("payload", &self.payload.as_ref().map(|_| ".."))
            .field("extra", &format_args!(".."))
            .finish()
    }
}

impl<T, X> fmt::Display for PushRefused<T, X> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "push refused: {}", self.error)
    }
}

impl<T, X> std::error::Error for PushRefused<T, X> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.error)
    }
}

/// Convert a native call's `HRESULT` into `Ok(())` or a wrapped
/// [`IoRingError`], following the `FAILED(hr)` convention (a negative
/// `HRESULT` is a failure; `S_OK` and `S_FALSE` are both non-negative
/// successes).
pub(crate) fn check(hr: HRESULT) -> io::Result<()> {
    if hr < 0 {
        Err(io::Error::other(IoRingError::new(hr)))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests;
